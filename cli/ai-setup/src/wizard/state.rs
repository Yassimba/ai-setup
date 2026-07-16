//! The wizard's state machine: stages, selection state, key and mouse
//! handling, and install progress. Everything here is terminal-free so the
//! whole flow is unit-testable; rendering lives in `render.rs`.

use crate::settings::{SettingSpec, SettingState, SettingsPaths};
use crate::{
    build_install_plan, InstallPlan, InstallReport, Platform, PrerequisiteStatus, Resource,
    ResourceKind, Runtime,
};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;

/// Everything the wizard needs to know up front; pure data so tests can
/// construct it without touching the file system.
pub struct Model {
    pub resources: Vec<Resource>,
    /// Per-resource flag: already present on this machine (plugin listed by
    /// `herdr plugin list`, package listed by `pi list`, skill in an agent
    /// tree).
    pub installed: Vec<bool>,
    pub settings: Vec<SettingSpec>,
    pub setting_states: Vec<SettingState>,
    /// Whether a Zed settings file exists on this machine; Zed tweaks are
    /// only pre-checked when there is a Zed to tweak.
    pub zed_present: bool,
    pub settings_paths: SettingsPaths,
    pub status: PrerequisiteStatus,
    pub platform: Platform,
    pub dry_run: bool,
}

#[derive(Debug)]
pub enum WizardOutcome {
    Cancelled,
    NothingSelected,
    DryRun(InstallPlan, Vec<String>),
    Installed(InstallReport),
}

/// What the event loop must do after a key or mouse event.
pub enum Action {
    Exit(WizardOutcome),
    StartInstall,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Focus {
    Categories,
    Skills,
}

pub(crate) struct Category {
    pub name: String,
    pub items: Vec<usize>,
}

/// A row of the Welcome runtime list.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum PickRow {
    /// The manager itself, offered as its own install.
    Runtime(Runtime),
    /// The manager is already on PATH; shown, but not actionable.
    InstalledRuntime(Runtime),
}

pub(crate) struct PickStage {
    pub title: &'static str,
    pub blurb: &'static str,
    /// The manager this stage's resources need; opting out of installing it
    /// on the Welcome screen hides the whole stage.
    pub runtime: Runtime,
    pub items: Vec<usize>,
    pub cursor: usize,
}

pub(crate) struct SkillsStage {
    pub categories: Vec<Category>,
    pub category_cursor: usize,
    pub skill_cursor: usize,
    pub focus: Focus,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) enum SettingRow {
    Header(String),
    Setting(usize),
}

pub(crate) struct SettingsStage {
    pub rows: Vec<SettingRow>,
    pub cursor: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecStatus {
    Pending,
    Running,
    Ok(String),
    Failed(String),
    Skipped(String),
}

#[derive(Clone, Debug)]
pub struct ExecItem {
    pub label: String,
    pub detail: String,
    pub status: ExecStatus,
}

pub(crate) struct InstallStage {
    pub items: Vec<ExecItem>,
    pub running: bool,
    pub report: Option<InstallReport>,
    pub tick: usize,
    pub scroll: u16,
}

/// The Welcome stage doubles as a quick-install surface: every runtime is
/// listed with its status, and the missing ones can be toggled right there.
pub(crate) struct WelcomeStage {
    pub rows: Vec<PickRow>,
    pub cursor: usize,
}

pub(crate) enum Stage {
    Welcome(WelcomeStage),
    Pick(PickStage),
    Skills(SkillsStage),
    Settings(SettingsStage),
    Review { scroll: u16 },
    Install(InstallStage),
}

impl Stage {
    pub fn title(&self) -> &str {
        match self {
            Self::Welcome(_) => "Welcome",
            Self::Pick(stage) => stage.title,
            Self::Skills(_) => "Skills",
            Self::Settings(_) => "Settings",
            Self::Review { .. } => "Review",
            Self::Install(_) => "Install",
        }
    }
}

/// Events sent by the install worker thread.
#[derive(Debug)]
pub enum InstallEvent {
    Status(usize, ExecStatus),
    Done(InstallReport),
}

/// The work handed to the install worker thread.
pub struct InstallJob {
    pub plan: InstallPlan,
    pub settings: Vec<SettingSpec>,
    pub paths: SettingsPaths,
}

/// Screen regions remembered from the last draw, for mouse hit-testing.
#[derive(Default)]
pub(crate) struct HitMap {
    pub sidebar: Rect,
    pub sidebar_rows: usize,
    pub back_button: Rect,
    pub next_button: Rect,
    /// (area, first-visible-row) of the primary and secondary lists.
    pub primary_list: Option<(Rect, usize)>,
    pub secondary_list: Option<(Rect, usize)>,
}

pub struct Wizard {
    pub(crate) model: Model,
    pub(crate) selected: Vec<bool>,
    /// Toggles for runtimes that are not yet installed.
    pub(crate) runtime_on: Vec<(Runtime, bool)>,
    pub(crate) setting_on: Vec<bool>,
    /// Settings the user explicitly toggled; contextual pre-checks leave
    /// those alone.
    pub(crate) setting_touched: Vec<bool>,
    pub(crate) stages: Vec<Stage>,
    pub(crate) stage_index: usize,
    pub(crate) max_visited: usize,
    pub(crate) hits: HitMap,
}

impl Wizard {
    pub fn new(model: Model) -> Self {
        let welcome_rows = [Runtime::Herdr, Runtime::Pi]
            .into_iter()
            .map(|runtime| {
                if runtime.installed(model.status) {
                    PickRow::InstalledRuntime(runtime)
                } else {
                    PickRow::Runtime(runtime)
                }
            })
            .collect::<Vec<_>>();
        let welcome_cursor = welcome_rows
            .iter()
            .position(|row| matches!(row, PickRow::Runtime(_)))
            .unwrap_or(0);
        let mut stages = vec![Stage::Welcome(WelcomeStage {
            rows: welcome_rows,
            cursor: welcome_cursor,
        })];
        for (kind, runtime, title, blurb) in [
            (
                ResourceKind::HerdrPlugin,
                Runtime::Herdr,
                "Herdr",
                "Plugins for Herdr, the terminal multiplexer for coding agents.",
            ),
            (
                ResourceKind::PiPackage,
                Runtime::Pi,
                "Pi",
                "Packages that extend the Pi coding agent.",
            ),
        ] {
            let items = indices_of_kind(&model.resources, &kind);
            if items.is_empty() {
                continue;
            }
            stages.push(Stage::Pick(PickStage {
                title,
                blurb,
                runtime,
                items,
                cursor: 0,
            }));
        }
        let categories = skill_categories(&model.resources);
        if !categories.is_empty() {
            stages.push(Stage::Skills(SkillsStage {
                categories,
                category_cursor: 0,
                skill_cursor: 0,
                focus: Focus::Categories,
            }));
        }
        if !model.settings.is_empty() {
            stages.push(Stage::Settings(SettingsStage {
                rows: setting_rows(&model.settings),
                cursor: first_setting_row(&setting_rows(&model.settings)),
            }));
        }
        stages.push(Stage::Review { scroll: 0 });
        stages.push(Stage::Install(InstallStage {
            items: Vec::new(),
            running: false,
            report: None,
            tick: 0,
            scroll: 0,
        }));

        let runtime_on = [Runtime::Herdr, Runtime::Pi]
            .into_iter()
            .filter(|runtime| !runtime.installed(model.status))
            .map(|runtime| (runtime, true))
            .collect();
        Self {
            selected: vec![false; model.resources.len()],
            setting_on: vec![false; model.settings.len()],
            setting_touched: vec![false; model.settings.len()],
            runtime_on,
            stages,
            stage_index: 0,
            max_visited: 0,
            model,
            hits: HitMap::default(),
        }
    }

    // ---- selection helpers -------------------------------------------------

    pub(crate) fn runtime_selected(&self, runtime: Runtime) -> bool {
        self.runtime_on
            .iter()
            .any(|(candidate, on)| *candidate == runtime && *on)
    }

    fn toggle_runtime(&mut self, runtime: Runtime) {
        if let Some(entry) = self
            .runtime_on
            .iter_mut()
            .find(|(candidate, _)| *candidate == runtime)
        {
            entry.1 = !entry.1;
        }
    }

    /// The manager a resource kind installs through; skills need none — the
    /// CLI copies them into the agent trees itself.
    fn runtime_for_kind(kind: &ResourceKind) -> Option<Runtime> {
        match kind {
            ResourceKind::HerdrPlugin => Some(Runtime::Herdr),
            ResourceKind::PiPackage => Some(Runtime::Pi),
            ResourceKind::Skill => None,
        }
    }

    /// An ecosystem is enabled when its manager is installed or the user
    /// left its Welcome-screen install toggle on.
    pub(crate) fn ecosystem_enabled(&self, runtime: Runtime) -> bool {
        runtime.installed(self.model.status) || self.runtime_selected(runtime)
    }

    /// Picks in a disabled ecosystem are kept (they come back if the user
    /// re-enables the runtime) but excluded from everything downstream.
    pub(crate) fn selection(&self) -> Vec<Resource> {
        self.model
            .resources
            .iter()
            .zip(&self.selected)
            .filter(|(resource, on)| {
                **on && Self::runtime_for_kind(&resource.kind)
                    .is_none_or(|runtime| self.ecosystem_enabled(runtime))
            })
            .map(|(resource, _)| resource.clone())
            .collect()
    }

    pub(crate) fn selected_runtimes(&self) -> Vec<Runtime> {
        self.runtime_on
            .iter()
            .filter(|(_, on)| *on)
            .map(|(runtime, _)| runtime)
            .copied()
            .collect()
    }

    pub(crate) fn selected_settings(&self) -> Vec<SettingSpec> {
        self.model
            .settings
            .iter()
            .zip(&self.setting_on)
            .filter(|(_, on)| **on)
            .map(|(spec, _)| spec.clone())
            .collect()
    }

    pub(crate) fn setting_applied(&self, index: usize) -> bool {
        self.model.setting_states[index] == SettingState::Applied
    }

    pub(crate) fn nothing_chosen(&self) -> bool {
        self.selection().is_empty()
            && self.selected_runtimes().is_empty()
            && self.selected_settings().is_empty()
    }

    pub(crate) fn plan(&self) -> Result<InstallPlan> {
        let selection = crate::expand_skill_dependencies(&self.model.resources, self.selection());
        build_install_plan(
            &selection,
            &self.selected_runtimes(),
            self.model.status,
            self.model.platform,
        )
    }

    pub(crate) fn selected_count(&self, items: &[usize]) -> usize {
        items.iter().filter(|&&index| self.selected[index]).count()
    }

    pub(crate) fn installed_count(&self, items: &[usize]) -> usize {
        items
            .iter()
            .filter(|&&index| self.model.installed[index])
            .count()
    }

    pub(crate) fn actionable(&self, items: &[usize]) -> Vec<usize> {
        items
            .iter()
            .copied()
            .filter(|&index| !self.model.installed[index])
            .collect()
    }

    pub(crate) fn total_selected(&self) -> usize {
        self.selection().len()
            + self.selected_runtimes().len()
            + self.setting_on.iter().filter(|on| **on).count()
    }

    /// Pre-check settings that pair with what the user picked, unless the
    /// user already touched them.
    fn precheck_settings(&mut self) {
        let selection = self.selection();
        for (index, spec) in self.model.settings.iter().enumerate() {
            if self.setting_touched[index] || self.setting_applied(index) {
                continue;
            }
            self.setting_on[index] = match &spec.related_resource {
                Some(resource_id) => {
                    (selection.iter().any(|resource| resource.id == *resource_id)
                        || self.model.resources.iter().zip(&self.model.installed).any(
                            |(resource, installed)| *installed && resource.id == *resource_id,
                        ))
                        // A Zed-targeting setting stays off without a Zed
                        // install, even when its plugin is selected.
                        && (!spec.requires_zed() || self.model.zed_present)
                }
                None => self.model.zed_present,
            };
        }
    }

    // ---- navigation --------------------------------------------------------

    pub(crate) fn install_running(&self) -> bool {
        matches!(
            &self.stages[self.stage_index],
            Stage::Install(stage) if stage.running
        )
    }

    fn review_index(&self) -> usize {
        self.stages.len() - 2
    }

    /// A stage is hidden when the user opted out of installing the runtime
    /// its resources need.
    pub(crate) fn stage_visible(&self, index: usize) -> bool {
        match &self.stages[index] {
            Stage::Pick(stage) => self.ecosystem_enabled(stage.runtime),
            _ => true,
        }
    }

    pub(crate) fn visible_stages(&self) -> Vec<usize> {
        (0..self.stages.len())
            .filter(|&index| self.stage_visible(index))
            .collect()
    }

    fn go_forward(&mut self) {
        // Review is the last stage reachable by plain navigation; Install
        // starts only from Review's confirm.
        let next =
            (self.stage_index + 1..=self.review_index()).find(|&index| self.stage_visible(index));
        if let Some(index) = next {
            self.stage_index = index;
            self.entered_stage();
        }
    }

    fn go_back(&mut self) {
        if matches!(self.stages[self.stage_index], Stage::Install(_)) {
            return;
        }
        if let Some(index) = (0..self.stage_index)
            .rev()
            .find(|&index| self.stage_visible(index))
        {
            self.stage_index = index;
        }
    }

    /// Jump to the `row`th *visible* stage, as shown in the sidebar.
    fn jump_to(&mut self, row: usize) {
        let Some(&index) = self.visible_stages().get(row) else {
            return;
        };
        let install_stage = self.stages.len() - 1;
        if index >= install_stage || index > self.max_visited {
            return;
        }
        self.stage_index = index;
        self.entered_stage();
    }

    fn entered_stage(&mut self) {
        self.max_visited = self.max_visited.max(self.stage_index);
        if let Stage::Settings(_) = &self.stages[self.stage_index] {
            self.precheck_settings();
        }
        if let Stage::Review { scroll } = &mut self.stages[self.stage_index] {
            *scroll = 0;
        }
    }

    fn confirm_review(&mut self) -> Option<Action> {
        if self.nothing_chosen() {
            return Some(Action::Exit(WizardOutcome::NothingSelected));
        }
        let Ok(plan) = self.plan() else {
            // The review screen explains why the plan cannot run; stay.
            return None;
        };
        if self.model.dry_run {
            let summary = self
                .selected_settings()
                .iter()
                .flat_map(|spec| {
                    let path = spec.target_path(&self.model.settings_paths).display();
                    spec.change_summary()
                        .into_iter()
                        .map(move |line| format!("{path}: {line}"))
                        .collect::<Vec<_>>()
                })
                .collect();
            return Some(Action::Exit(WizardOutcome::DryRun(plan, summary)));
        }
        self.stage_index = self.stages.len() - 1;
        self.max_visited = self.stage_index;
        Some(Action::StartInstall)
    }

    // ---- install execution -------------------------------------------------

    /// Build the worker job and seed the install screen's step list.
    pub fn begin_install(&mut self) -> Result<InstallJob> {
        let plan = self.plan()?;
        let settings = self.selected_settings();
        let mut items = Vec::new();
        for step in &plan.prerequisites {
            items.push(ExecItem {
                label: format!("Install {}", step.target),
                detail: step.action.display(),
                status: ExecStatus::Pending,
            });
        }
        for step in &plan.resources {
            // Show the human name from the catalog, not the resource id.
            let name = self
                .model
                .resources
                .iter()
                .find(|resource| resource.id == step.target)
                .map(|resource| resource.label.as_str())
                .unwrap_or(&step.target);
            items.push(ExecItem {
                label: format!("Install {name}"),
                detail: step.action.display(),
                status: ExecStatus::Pending,
            });
        }
        for spec in &settings {
            items.push(ExecItem {
                label: format!("Configure {}", spec.label),
                detail: spec
                    .target_path(&self.model.settings_paths)
                    .display()
                    .to_string(),
                status: ExecStatus::Pending,
            });
        }
        let Stage::Install(stage) = &mut self.stages[self.stage_index] else {
            anyhow::bail!("install started outside the install stage");
        };
        stage.items = items;
        stage.running = true;
        Ok(InstallJob {
            plan,
            settings,
            paths: self.model.settings_paths.clone(),
        })
    }

    pub fn handle_install_event(&mut self, event: InstallEvent) {
        let Stage::Install(stage) = &mut self.stages[self.stage_index] else {
            return;
        };
        match event {
            InstallEvent::Status(index, status) => {
                if let Some(item) = stage.items.get_mut(index) {
                    item.status = status;
                }
            }
            InstallEvent::Done(report) => {
                stage.running = false;
                stage.report = Some(report);
            }
        }
    }

    pub fn tick(&mut self) {
        if let Stage::Install(stage) = &mut self.stages[self.stage_index] {
            stage.tick = stage.tick.wrapping_add(1);
        }
    }

    // ---- input -------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let is_ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if self.install_running() {
            // Interrupting a running install would leave managers half
            // configured; the worker finishes, then keys work again.
            return None;
        }
        if is_ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
            if let Stage::Install(stage) = &self.stages[self.stage_index] {
                if let Some(report) = &stage.report {
                    return Some(Action::Exit(WizardOutcome::Installed(report.clone())));
                }
            }
            return Some(Action::Exit(WizardOutcome::Cancelled));
        }
        if key.code == KeyCode::Enter {
            return self.handle_enter();
        }

        // -1 steps back, +1 steps forward; resolved after the borrow of the
        // current stage ends.
        let mut navigate = 0i8;
        match &mut self.stages[self.stage_index] {
            Stage::Welcome(stage) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => stage.cursor = stage.cursor.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    stage.cursor = (stage.cursor + 1).min(stage.rows.len() - 1);
                }
                KeyCode::Char(' ') => {
                    let row = stage.rows[stage.cursor];
                    self.toggle_pick_row(row);
                }
                KeyCode::Char('a') => {
                    let rows = stage.rows.clone();
                    self.toggle_pick_all(&rows);
                }
                KeyCode::Right | KeyCode::Char('l') => navigate = 1,
                _ => {}
            },
            Stage::Pick(stage) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => stage.cursor = stage.cursor.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    stage.cursor = (stage.cursor + 1).min(stage.items.len() - 1);
                }
                KeyCode::Char(' ') => {
                    let index = stage.items[stage.cursor];
                    if !self.model.installed[index] {
                        self.selected[index] = !self.selected[index];
                    }
                }
                KeyCode::Char('a') => {
                    toggle_group(&mut self.selected, &stage.items, &self.model.installed);
                }
                KeyCode::Right | KeyCode::Char('l') => navigate = 1,
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Char('b') => {
                    navigate = -1;
                }
                _ => {}
            },
            Stage::Skills(stage) => {
                let category = &stage.categories[stage.category_cursor];
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => match stage.focus {
                        Focus::Categories => {
                            stage.category_cursor = stage.category_cursor.saturating_sub(1);
                            stage.skill_cursor = 0;
                        }
                        Focus::Skills => stage.skill_cursor = stage.skill_cursor.saturating_sub(1),
                    },
                    KeyCode::Down | KeyCode::Char('j') => match stage.focus {
                        Focus::Categories => {
                            stage.category_cursor =
                                (stage.category_cursor + 1).min(stage.categories.len() - 1);
                            stage.skill_cursor = 0;
                        }
                        Focus::Skills => {
                            stage.skill_cursor =
                                (stage.skill_cursor + 1).min(category.items.len() - 1);
                        }
                    },
                    KeyCode::Char(' ') => match stage.focus {
                        Focus::Categories => {
                            toggle_group(&mut self.selected, &category.items, &self.model.installed)
                        }
                        Focus::Skills => {
                            let index = category.items[stage.skill_cursor];
                            if !self.model.installed[index] {
                                self.selected[index] = !self.selected[index];
                            }
                        }
                    },
                    KeyCode::Char('a') => {
                        toggle_group(&mut self.selected, &category.items, &self.model.installed)
                    }
                    KeyCode::Char('A') => {
                        let all = stage
                            .categories
                            .iter()
                            .flat_map(|category| category.items.iter().copied())
                            .collect::<Vec<_>>();
                        toggle_group(&mut self.selected, &all, &self.model.installed);
                    }
                    KeyCode::Tab => {
                        stage.focus = match stage.focus {
                            Focus::Categories => Focus::Skills,
                            Focus::Skills => Focus::Categories,
                        };
                    }
                    KeyCode::Right | KeyCode::Char('l') => stage.focus = Focus::Skills,
                    KeyCode::Left | KeyCode::Char('h') => {
                        if stage.focus == Focus::Skills {
                            stage.focus = Focus::Categories;
                        } else {
                            navigate = -1;
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('b') => navigate = -1,
                    _ => {}
                }
            }
            Stage::Settings(stage) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    stage.cursor = previous_setting_row(&stage.rows, stage.cursor);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    stage.cursor = next_setting_row(&stage.rows, stage.cursor);
                }
                KeyCode::Char(' ') => {
                    if let SettingRow::Setting(index) = stage.rows[stage.cursor] {
                        if self.model.setting_states[index] != SettingState::Applied {
                            self.setting_on[index] = !self.setting_on[index];
                            self.setting_touched[index] = true;
                        }
                    }
                }
                KeyCode::Char('a') => {
                    let actionable = (0..self.model.settings.len())
                        .filter(|&index| !self.setting_applied(index))
                        .collect::<Vec<_>>();
                    let all_on = actionable.iter().all(|&index| self.setting_on[index]);
                    for index in actionable {
                        self.setting_on[index] = !all_on;
                        self.setting_touched[index] = true;
                    }
                }
                KeyCode::Right | KeyCode::Char('l') => navigate = 1,
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Char('b') => {
                    navigate = -1;
                }
                _ => {}
            },
            Stage::Review { scroll } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Char('b') => {
                    navigate = -1;
                }
                _ => {}
            },
            Stage::Install(stage) => match key.code {
                KeyCode::Up | KeyCode::Char('k') => stage.scroll = stage.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => stage.scroll = stage.scroll.saturating_add(1),
                _ => {}
            },
        }
        match navigate {
            -1 => self.go_back(),
            1 => self.go_forward(),
            _ => {}
        }
        None
    }

    fn handle_enter(&mut self) -> Option<Action> {
        match &self.stages[self.stage_index] {
            Stage::Review { .. } => self.confirm_review(),
            Stage::Install(stage) => stage
                .report
                .as_ref()
                .map(|report| Action::Exit(WizardOutcome::Installed(report.clone()))),
            _ => {
                self.go_forward();
                None
            }
        }
    }

    fn toggle_pick_row(&mut self, row: PickRow) {
        match row {
            PickRow::Runtime(runtime) => self.toggle_runtime(runtime),
            PickRow::InstalledRuntime(_) => {}
        }
    }

    fn toggle_pick_all(&mut self, rows: &[PickRow]) {
        let all_on = rows.iter().all(|row| match row {
            PickRow::Runtime(runtime) => self.runtime_selected(*runtime),
            PickRow::InstalledRuntime(_) => true,
        });
        for row in rows {
            if let PickRow::Runtime(runtime) = row {
                if let Some(entry) = self
                    .runtime_on
                    .iter_mut()
                    .find(|(candidate, _)| candidate == runtime)
                {
                    entry.1 = !all_on;
                }
            }
        }
    }

    // ---- mouse -------------------------------------------------------------

    pub fn handle_click(&mut self, column: u16, row: u16) -> Option<Action> {
        if self.install_running() {
            return None;
        }
        if contains(self.hits.back_button, column, row) {
            self.go_back();
            return None;
        }
        if contains(self.hits.next_button, column, row) {
            return self.handle_enter();
        }
        if contains(self.hits.sidebar, column, row) {
            let index = row.saturating_sub(self.hits.sidebar.y + 1) as usize;
            if index < self.hits.sidebar_rows {
                self.jump_to(index);
            }
            return None;
        }
        if let Some((area, offset)) = self.hits.primary_list {
            if contains(area, column, row) {
                let index = offset + row.saturating_sub(area.y + 1) as usize;
                self.click_primary(index);
                return None;
            }
        }
        if let Some((area, offset)) = self.hits.secondary_list {
            if contains(area, column, row) {
                let index = offset + row.saturating_sub(area.y + 1) as usize;
                self.click_secondary(index);
            }
        }
        None
    }

    fn click_primary(&mut self, index: usize) {
        match &mut self.stages[self.stage_index] {
            Stage::Welcome(stage) => {
                if index < stage.rows.len() {
                    stage.cursor = index;
                    let row = stage.rows[index];
                    self.toggle_pick_row(row);
                }
            }
            Stage::Pick(stage) => {
                if index < stage.items.len() {
                    stage.cursor = index;
                    let item = stage.items[index];
                    if !self.model.installed[item] {
                        self.selected[item] = !self.selected[item];
                    }
                }
            }
            Stage::Skills(stage) => {
                if index < stage.categories.len() {
                    stage.focus = Focus::Categories;
                    stage.category_cursor = index;
                    stage.skill_cursor = 0;
                }
            }
            Stage::Settings(stage) => {
                if let Some(SettingRow::Setting(setting)) = stage.rows.get(index).cloned() {
                    stage.cursor = index;
                    if self.model.setting_states[setting] != SettingState::Applied {
                        self.setting_on[setting] = !self.setting_on[setting];
                        self.setting_touched[setting] = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn click_secondary(&mut self, index: usize) {
        if let Stage::Skills(stage) = &mut self.stages[self.stage_index] {
            let category = &stage.categories[stage.category_cursor];
            if index < category.items.len() {
                stage.focus = Focus::Skills;
                stage.skill_cursor = index;
                let item = category.items[index];
                if !self.model.installed[item] {
                    self.selected[item] = !self.selected[item];
                }
            }
        }
    }

    pub fn handle_scroll(&mut self, down: bool) {
        let code = if down { KeyCode::Down } else { KeyCode::Up };
        let _ = self.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

fn indices_of_kind(resources: &[Resource], kind: &ResourceKind) -> Vec<usize> {
    resources
        .iter()
        .enumerate()
        .filter(|(_, resource)| resource.kind == *kind)
        .map(|(index, _)| index)
        .collect()
}

fn skill_categories(resources: &[Resource]) -> Vec<Category> {
    let mut categories: Vec<Category> = Vec::new();
    for (index, resource) in resources.iter().enumerate() {
        if resource.kind != ResourceKind::Skill {
            continue;
        }
        match categories
            .iter_mut()
            .find(|category| category.name == resource.group)
        {
            Some(category) => category.items.push(index),
            None => categories.push(Category {
                name: resource.group.clone(),
                items: vec![index],
            }),
        }
    }
    categories
}

fn setting_rows(settings: &[SettingSpec]) -> Vec<SettingRow> {
    let mut rows = Vec::new();
    let mut group: Option<&str> = None;
    for (index, spec) in settings.iter().enumerate() {
        if group != Some(spec.group.as_str()) {
            rows.push(SettingRow::Header(spec.group.clone()));
            group = Some(spec.group.as_str());
        }
        rows.push(SettingRow::Setting(index));
    }
    rows
}

fn first_setting_row(rows: &[SettingRow]) -> usize {
    rows.iter()
        .position(|row| matches!(row, SettingRow::Setting(_)))
        .unwrap_or(0)
}

fn next_setting_row(rows: &[SettingRow], cursor: usize) -> usize {
    rows.iter()
        .enumerate()
        .skip(cursor + 1)
        .find(|(_, row)| matches!(row, SettingRow::Setting(_)))
        .map_or(cursor, |(index, _)| index)
}

fn previous_setting_row(rows: &[SettingRow], cursor: usize) -> usize {
    rows.iter()
        .enumerate()
        .take(cursor)
        .rev()
        .find(|(_, row)| matches!(row, SettingRow::Setting(_)))
        .map_or(cursor, |(index, _)| index)
}

/// Select-all over a group, skipping resources that are already installed.
fn toggle_group(selected: &mut [bool], items: &[usize], installed: &[bool]) {
    let actionable = items
        .iter()
        .copied()
        .filter(|&index| !installed[index])
        .collect::<Vec<_>>();
    let all_selected = actionable.iter().all(|&index| selected[index]);
    for index in actionable {
        selected[index] = !all_selected;
    }
}
