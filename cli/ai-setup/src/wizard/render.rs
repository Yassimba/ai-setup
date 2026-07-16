//! All drawing for the wizard. The layout is a GUI-style installer: a
//! persistent stage sidebar on the left, the active stage's panel on the
//! right, a header with overall progress, and a footer with key hints and
//! clickable Back/Next buttons.

use super::state::{
    ExecStatus, Focus, HitMap, InstallStage, PickRow, PickStage, SettingRow, SettingsStage,
    SkillsStage, Stage, WelcomeStage, Wizard,
};
use crate::settings::SettingSpec;
use crate::{Resource, Runtime};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Gauge, List, ListItem, ListState, Padding, Paragraph, Wrap,
};
use ratatui::Frame;

pub(crate) const ACCENT: Color = Color::Cyan;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;
const SELECTED_MARK: &str = "◉";
const UNSELECTED_MARK: &str = "○";
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

type ListHit = Option<(Rect, usize)>;

impl Wizard {
    pub fn draw(&mut self, frame: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [sidebar, content] =
            Layout::horizontal([Constraint::Length(20), Constraint::Min(1)]).areas(body);

        self.hits = HitMap {
            sidebar,
            sidebar_rows: self.visible_stages().len(),
            ..HitMap::default()
        };
        self.render_header(frame, header);
        self.render_sidebar(frame, sidebar);
        let (primary, secondary) = match &self.stages[self.stage_index] {
            Stage::Welcome(stage) => self.render_welcome(frame, content, stage),
            Stage::Pick(stage) => self.render_pick(frame, content, stage),
            Stage::Skills(stage) => self.render_skills(frame, content, stage),
            Stage::Settings(stage) => self.render_settings(frame, content, stage),
            Stage::Review { scroll } => {
                self.render_review(frame, content, *scroll);
                (None, None)
            }
            Stage::Install(stage) => {
                self.render_install(frame, content, stage);
                (None, None)
            }
        };
        self.hits.primary_list = primary;
        self.hits.secondary_list = secondary;
        self.render_footer(frame, footer);
    }

    // ---- chrome ------------------------------------------------------------

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(" ⚙ ai-setup ", Style::new().fg(ACCENT).bold()),
            Span::styled(concat!("v", env!("CARGO_PKG_VERSION")), Style::new().dim()),
        ]);
        frame.render_widget(Paragraph::new(title), area);
        let visible = self.visible_stages();
        let step = visible
            .iter()
            .position(|&index| index == self.stage_index)
            .unwrap_or(0)
            + 1;
        let status = Line::from(vec![
            Span::styled(
                format!("{} selected", self.total_selected()),
                Style::new().fg(OK),
            ),
            Span::styled(
                format!("  ·  step {step}/{} ", visible.len()),
                Style::new().dim(),
            ),
        ]);
        frame.render_widget(Paragraph::new(status).alignment(Alignment::Right), area);
    }

    fn render_sidebar(&self, frame: &mut Frame, area: Rect) {
        let items = self
            .visible_stages()
            .into_iter()
            .enumerate()
            .map(|(position, index)| {
                let (mark, style) = match index.cmp(&self.stage_index) {
                    std::cmp::Ordering::Less => ("✓", Style::new().fg(OK)),
                    std::cmp::Ordering::Equal => ("▶", Style::new().fg(ACCENT).bold()),
                    std::cmp::Ordering::Greater if index <= self.max_visited => ("·", Style::new()),
                    std::cmp::Ordering::Greater => ("·", Style::new().dim()),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {mark} "), style),
                    Span::styled(
                        format!("{} {}", position + 1, self.stages[index].title()),
                        style,
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        let list = List::new(items).block(bordered(" Steps ", false));
        frame.render_widget(list, area);
    }

    fn render_footer(&mut self, frame: &mut Frame, area: Rect) {
        let hint = match &self.stages[self.stage_index] {
            Stage::Welcome(_) => " space toggle · ↑↓ move · enter start · q quit",
            Stage::Pick(_) => {
                " space toggle · a all · ↑↓/click move · enter next · ← back · q quit"
            }
            Stage::Skills(_) => {
                " space toggle · a category · A everything · tab panes · enter next · ← back"
            }
            Stage::Settings(_) => {
                " space toggle · a all · ↑↓ move · enter review · ← back · q quit"
            }
            Stage::Review { .. } => " enter install · ↑↓ scroll · ← back · q quit",
            Stage::Install(stage) if stage.running => " installing… please wait",
            Stage::Install(_) => " enter finish · ↑↓ scroll",
        };
        let [hint_area, back_area, _, next_area, _] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(10),
            Constraint::Length(1),
            Constraint::Length(13),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, Style::new().dim()))),
            hint_area,
        );

        let (back_enabled, next_label, next_enabled) = self.button_states();
        let back_style = if back_enabled {
            Style::new().fg(ACCENT)
        } else {
            Style::new().dim()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("[ ◂ Back ]", back_style))),
            back_area,
        );
        let next_style = if next_enabled {
            Style::new().fg(Color::Black).bg(ACCENT).bold()
        } else {
            Style::new().dim()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("[ {next_label} ▸ ]"),
                next_style,
            ))),
            next_area,
        );
        if back_enabled {
            self.hits.back_button = back_area;
        }
        if next_enabled {
            self.hits.next_button = next_area;
        }
    }

    fn button_states(&self) -> (bool, &'static str, bool) {
        match &self.stages[self.stage_index] {
            Stage::Welcome(_) => (false, "Start", true),
            Stage::Pick(_) | Stage::Skills(_) | Stage::Settings(_) => (true, "Next", true),
            Stage::Review { .. } => (
                true,
                "Install",
                self.nothing_chosen() || self.plan().is_ok(),
            ),
            Stage::Install(stage) => (false, "Finish", stage.report.is_some()),
        }
    }

    // ---- stages ------------------------------------------------------------

    fn render_welcome(
        &self,
        frame: &mut Frame,
        area: Rect,
        stage: &WelcomeStage,
    ) -> (ListHit, ListHit) {
        let block = bordered(" Welcome ", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let column = centered(inner, 66);
        let list_height = stage.rows.len() as u16 + 2;
        let [header_area, runtimes_area, footer_area] = Layout::vertical([
            Constraint::Length(8),
            Constraint::Length(list_height),
            Constraint::Min(1),
        ])
        .areas(column);

        let counts = [
            (
                "Herdr plugins",
                count_kind(&self.model.resources, crate::ResourceKind::HerdrPlugin),
            ),
            (
                "Pi packages",
                count_kind(&self.model.resources, crate::ResourceKind::PiPackage),
            ),
            (
                "skills",
                count_kind(&self.model.resources, crate::ResourceKind::Skill),
            ),
            ("settings", self.model.settings.len()),
        ];
        let header = vec![
            Line::from(""),
            Line::styled("▄▀█ █   █▀ █▀▀ ▀█▀ █░█ █▀█", Style::new().fg(ACCENT).bold()),
            Line::styled("█▀█ █   ▄█ ██▄ ░█░ █▄█ █▀▀", Style::new().fg(ACCENT).bold()),
            Line::from(""),
            Line::styled(
                "Yassimba's curated agent setup",
                Style::new().add_modifier(Modifier::ITALIC),
            ),
            Line::from(""),
            Line::from(Span::raw(
                counts
                    .iter()
                    .filter(|(_, count)| *count > 0)
                    .map(|(label, count)| format!("{count} {label}"))
                    .collect::<Vec<_>>()
                    .join("  ·  "),
            )),
        ];
        frame.render_widget(
            Paragraph::new(header).alignment(Alignment::Center),
            header_area,
        );

        let missing = stage
            .rows
            .iter()
            .filter(|row| matches!(row, PickRow::Runtime(_)))
            .count();
        let title = if missing == 0 {
            " Tools · all installed ".to_owned()
        } else {
            format!(" Tools · {missing} to install ")
        };
        let items = stage
            .rows
            .iter()
            .map(|row| self.pick_row_item(row))
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(bordered(&title, true))
            .highlight_style(Style::new().bg(Color::DarkGray));
        let mut state = ListState::default().with_selected(Some(stage.cursor));
        frame.render_stateful_widget(list, runtimes_area, &mut state);

        let highlighted = match stage.rows[stage.cursor] {
            PickRow::Runtime(runtime) | PickRow::InstalledRuntime(runtime) => runtime,
        };
        let mut footer = vec![
            Line::styled(runtime_blurb(highlighted), Style::new().dim()),
            Line::from(""),
        ];
        if !self.model.status.npm {
            footer.push(Line::styled(
                "○ npm is missing — install Node.js before picking Pi.",
                Style::new().fg(WARN),
            ));
            footer.push(Line::from(""));
        }
        footer.push(Line::styled(
            "Pick what you want, review the plan, watch it install.",
            Style::new().dim(),
        ));
        footer.push(Line::from(""));
        footer.push(Line::from(vec![
            Span::raw("Space or click to toggle · "),
            Span::styled("Enter", Style::new().fg(ACCENT).bold()),
            Span::raw(" to start."),
        ]));
        frame.render_widget(
            Paragraph::new(footer).alignment(Alignment::Center),
            footer_area,
        );
        (Some((runtimes_area, 0)), None)
    }

    fn render_pick(&self, frame: &mut Frame, area: Rect, stage: &PickStage) -> (ListHit, ListHit) {
        let [list_area, details_area] =
            Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
                .areas(area);

        let items = stage
            .items
            .iter()
            .map(|&index| self.resource_item(index))
            .collect::<Vec<_>>();
        let title = list_title(
            stage.title,
            self.selected_count(&stage.items),
            self.installed_count(&stage.items),
            stage.items.len(),
        );
        let offset = list_offset(
            stage.items.len(),
            list_area.height.saturating_sub(2),
            stage.cursor,
        );
        let list = List::new(items)
            .block(bordered(&title, true))
            .highlight_style(Style::new().bg(Color::DarkGray));
        let mut state = ListState::default()
            .with_selected(Some(stage.cursor))
            .with_offset(offset);
        frame.render_stateful_widget(list, list_area, &mut state);

        self.render_pick_details(frame, details_area, stage);
        (Some((list_area, offset)), None)
    }

    fn pick_row_item(&self, row: &PickRow) -> ListItem<'_> {
        match row {
            PickRow::InstalledRuntime(runtime) => ListItem::new(Line::from(vec![
                Span::styled(" ✓ ", Style::new().fg(OK)),
                Span::styled(runtime_name(*runtime), Style::new().dim()),
                Span::styled(" — installed", Style::new().dim()),
            ])),
            PickRow::Runtime(runtime) => {
                let on = self.runtime_selected(*runtime);
                let (mark, style) = mark_for(on);
                let needs_it = match runtime {
                    Runtime::Pi => "packages need Pi to be installed",
                    Runtime::Herdr => "plugins need Herdr to be installed",
                };
                let note = if on {
                    Span::styled(" — will be installed", Style::new().fg(OK))
                } else {
                    Span::styled(format!(" — skipped ({needs_it})"), Style::new().fg(WARN))
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {mark} "), style),
                    Span::styled(runtime_name(*runtime), Style::new().bold()),
                    note,
                ]))
            }
        }
    }

    fn render_pick_details(&self, frame: &mut Frame, area: Rect, stage: &PickStage) {
        let mut lines = vec![Line::styled(stage.blurb, Style::new().dim())];
        if !stage.runtime.installed(self.model.status) {
            lines.push(Line::styled(
                format!(
                    "⚙ {} isn't installed yet — it goes in first.",
                    runtime_name(stage.runtime)
                ),
                Style::new().fg(WARN),
            ));
        }
        lines.push(Line::from(""));
        lines.extend(self.resource_details(stage.items[stage.cursor]));
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(bordered(" Details ", false).padding(Padding::horizontal(1))),
            area,
        );
    }

    fn resource_details(&self, index: usize) -> Vec<Line<'_>> {
        let resource = &self.model.resources[index];
        let mut lines = vec![
            Line::styled(resource.label.clone(), Style::new().bold().fg(ACCENT)),
            Line::styled(
                format!("{} · {}", resource.kind, resource.group),
                Style::new().dim(),
            ),
            Line::from(""),
            Line::from(resource.description.clone()),
            Line::from(""),
            Line::styled(
                format!("target  {}", resource.install_target),
                Style::new().dim(),
            ),
            Line::from(""),
            Line::styled(
                format!("next  {}", resource.next_action),
                Style::new().dim(),
            ),
        ];
        if self.model.installed[index] {
            lines.push(Line::from(""));
            lines.push(Line::styled("Already installed.", Style::new().fg(OK)));
        }
        lines
    }

    fn render_skills(
        &self,
        frame: &mut Frame,
        area: Rect,
        stage: &SkillsStage,
    ) -> (ListHit, ListHit) {
        let [top, details_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(6)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).areas(top);

        let categories = stage
            .categories
            .iter()
            .map(|category| {
                let count = self.selected_count(&category.items);
                let actionable = self.actionable(&category.items).len();
                let count_style = if count > 0 {
                    Style::new().fg(OK)
                } else {
                    Style::new().dim()
                };
                let mut spans = vec![Span::raw(format!(" {} ", category.name))];
                if actionable == 0 {
                    spans.push(Span::styled("✓", Style::new().fg(OK)));
                } else {
                    spans.push(Span::styled(format!("{count}/{actionable}"), count_style));
                }
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>();
        let category_offset = list_offset(
            stage.categories.len(),
            left.height.saturating_sub(2),
            stage.category_cursor,
        );
        let category_list = List::new(categories)
            .block(bordered(" Categories ", stage.focus == Focus::Categories))
            .highlight_style(highlight(stage.focus == Focus::Categories));
        let mut category_state = ListState::default()
            .with_selected(Some(stage.category_cursor))
            .with_offset(category_offset);
        frame.render_stateful_widget(category_list, left, &mut category_state);

        let category = &stage.categories[stage.category_cursor];
        let skills = category
            .items
            .iter()
            .map(|&index| self.resource_item(index))
            .collect::<Vec<_>>();
        let title = list_title(
            &category.name,
            self.selected_count(&category.items),
            self.installed_count(&category.items),
            category.items.len(),
        );
        let skill_offset = list_offset(
            category.items.len(),
            right.height.saturating_sub(2),
            stage.skill_cursor,
        );
        let skill_list = List::new(skills)
            .block(bordered(&title, stage.focus == Focus::Skills))
            .highlight_style(highlight(stage.focus == Focus::Skills));
        let mut skill_state = ListState::default()
            .with_selected(Some(stage.skill_cursor))
            .with_offset(skill_offset);
        frame.render_stateful_widget(skill_list, right, &mut skill_state);

        let skill_index = category.items[stage.skill_cursor];
        let skill = &self.model.resources[skill_index];
        let installed_note = if self.model.installed[skill_index] {
            Span::styled("  ✓ installed", Style::new().fg(OK))
        } else {
            Span::raw("")
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(skill.label.clone(), Style::new().bold().fg(ACCENT)),
                    Span::styled(format!("  {}", skill.group), Style::new().dim()),
                    installed_note,
                ]),
                Line::from(skill.description.clone()),
                Line::styled(format!("next  {}", skill.next_action), Style::new().dim()),
            ])
            .wrap(Wrap { trim: true })
            .block(bordered(" Details ", false).padding(Padding::horizontal(1))),
            details_area,
        );
        (Some((left, category_offset)), Some((right, skill_offset)))
    }

    fn render_settings(
        &self,
        frame: &mut Frame,
        area: Rect,
        stage: &SettingsStage,
    ) -> (ListHit, ListHit) {
        let [list_area, details_area] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(area);

        let items = stage
            .rows
            .iter()
            .map(|row| match row {
                SettingRow::Header(group) => ListItem::new(Line::from(Span::styled(
                    format!(" {group}"),
                    Style::new().bold().underlined(),
                ))),
                SettingRow::Setting(index) => {
                    let spec = &self.model.settings[*index];
                    if self.setting_applied(*index) {
                        ListItem::new(Line::from(vec![
                            Span::styled(" ✓ ", Style::new().fg(OK)),
                            Span::styled(spec.label.clone(), Style::new().dim()),
                            Span::styled(" — already set", Style::new().dim()),
                        ]))
                    } else {
                        let (mark, style) = mark_for(self.setting_on[*index]);
                        ListItem::new(Line::from(vec![
                            Span::styled(format!(" {mark} "), style),
                            Span::raw(spec.label.clone()),
                        ]))
                    }
                }
            })
            .collect::<Vec<_>>();
        let on_count = self.setting_on.iter().filter(|on| **on).count();
        let title = format!(
            " Settings · {on_count}/{} selected ",
            self.model.settings.len()
        );
        let offset = list_offset(
            stage.rows.len(),
            list_area.height.saturating_sub(2),
            stage.cursor,
        );
        let list = List::new(items)
            .block(bordered(&title, true))
            .highlight_style(Style::new().bg(Color::DarkGray));
        let mut state = ListState::default()
            .with_selected(Some(stage.cursor))
            .with_offset(offset);
        frame.render_stateful_widget(list, list_area, &mut state);

        if let SettingRow::Setting(index) = stage.rows[stage.cursor] {
            self.render_setting_details(frame, details_area, &self.model.settings[index], index);
        }
        (Some((list_area, offset)), None)
    }

    fn render_setting_details(
        &self,
        frame: &mut Frame,
        area: Rect,
        spec: &SettingSpec,
        index: usize,
    ) {
        let mut lines = vec![
            Line::styled(spec.label.clone(), Style::new().bold().fg(ACCENT)),
            Line::styled(spec.group.clone(), Style::new().dim()),
            Line::from(""),
            Line::from(spec.description.clone()),
            Line::from(""),
            Line::styled(
                format!(
                    "file  {}",
                    spec.target_path(&self.model.settings_paths).display()
                ),
                Style::new().dim(),
            ),
            Line::from(""),
        ];
        for change in spec.change_summary() {
            lines.push(Line::styled(format!("+ {change}"), Style::new().fg(OK)));
        }
        if self.setting_applied(index) {
            lines.push(Line::from(""));
            lines.push(Line::styled("Already set.", Style::new().fg(OK)));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(bordered(" Details ", false).padding(Padding::horizontal(1))),
            area,
        );
    }

    fn render_review(&self, frame: &mut Frame, area: Rect, scroll: u16) {
        let mut lines = Vec::new();
        if self.nothing_chosen() {
            lines.push(Line::from(
                "Nothing picked yet. Enter leaves without changes; ← goes back.",
            ));
        } else {
            let runtimes = self.selected_runtimes();
            if !runtimes.is_empty() {
                lines.push(Line::styled("Tools", Style::new().bold()));
                for runtime in &runtimes {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {SELECTED_MARK} "), Style::new().fg(OK)),
                        Span::raw(runtime_name(*runtime)),
                    ]));
                }
                lines.push(Line::from(""));
            }
            let selection = self.selection();
            for (kind, title) in [
                (crate::ResourceKind::HerdrPlugin, "Herdr plugins"),
                (crate::ResourceKind::PiPackage, "Pi packages"),
                (crate::ResourceKind::Skill, "Skills"),
            ] {
                let of_kind = selection
                    .iter()
                    .filter(|resource| resource.kind == kind)
                    .collect::<Vec<_>>();
                if of_kind.is_empty() {
                    continue;
                }
                lines.push(Line::styled(
                    format!("{title} ({})", of_kind.len()),
                    Style::new().bold(),
                ));
                for resource in of_kind {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {SELECTED_MARK} "), Style::new().fg(OK)),
                        Span::raw(resource.label.clone()),
                        Span::styled(format!("  {}", resource.group), Style::new().dim()),
                    ]));
                }
                lines.push(Line::from(""));
            }
            let settings = self.selected_settings();
            if !settings.is_empty() {
                lines.push(Line::styled(
                    format!("Settings ({})", settings.len()),
                    Style::new().bold(),
                ));
                for spec in &settings {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {SELECTED_MARK} "), Style::new().fg(OK)),
                        Span::raw(spec.label.clone()),
                        Span::styled(
                            format!(
                                "  → {}",
                                spec.target_path(&self.model.settings_paths).display()
                            ),
                            Style::new().dim(),
                        ),
                    ]));
                }
                lines.push(Line::from(""));
            }
            match self.plan() {
                Ok(plan) => {
                    lines.push(Line::styled("What will run", Style::new().bold()));
                    for step in &plan.prerequisites {
                        lines.push(Line::styled(
                            format!(
                                "  ! {} goes in first: {}",
                                step.target,
                                step.action.display()
                            ),
                            Style::new().fg(WARN),
                        ));
                    }
                    for step in &plan.resources {
                        lines.push(Line::from(format!("  $ {}", step.action.display())));
                    }
                    for spec in &settings {
                        lines.push(Line::from(format!(
                            "  ~ updates {}",
                            spec.target_path(&self.model.settings_paths).display()
                        )));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::styled(
                        if self.model.dry_run {
                            "Dry run: Enter prints this plan and exits."
                        } else {
                            "Enter runs it."
                        },
                        Style::new().fg(OK),
                    ));
                }
                Err(error) => {
                    lines.push(Line::styled(
                        format!("Cannot install: {error}"),
                        Style::new().fg(ERR),
                    ));
                }
            }
        }
        let paragraph = Paragraph::new(lines)
            .block(bordered(" Review ", true).padding(Padding::horizontal(1)))
            .scroll((scroll, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_install(&self, frame: &mut Frame, area: Rect, stage: &InstallStage) {
        let summary_height = if stage.report.is_some() { 7 } else { 0 };
        let [gauge_area, steps_area, summary_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(summary_height),
        ])
        .areas(area);

        let total = stage.items.len().max(1);
        let done = stage
            .items
            .iter()
            .filter(|item| !matches!(item.status, ExecStatus::Pending | ExecStatus::Running))
            .count();
        let failed = stage
            .items
            .iter()
            .any(|item| matches!(item.status, ExecStatus::Failed(_)));
        let gauge_color = if failed { ERR } else { OK };
        frame.render_widget(
            Gauge::default()
                .block(bordered(" Progress ", true))
                .gauge_style(Style::new().fg(gauge_color).bg(Color::DarkGray))
                .ratio(done as f64 / total as f64)
                .label(format!("{done}/{total}")),
            gauge_area,
        );

        let spinner = SPINNER[stage.tick % SPINNER.len()];
        let items = stage
            .items
            .iter()
            .map(|item| {
                let (mark, style, note) = match &item.status {
                    ExecStatus::Pending => ("○".into(), Style::new().dim(), String::new()),
                    ExecStatus::Running => (
                        spinner.to_string(),
                        Style::new().fg(ACCENT),
                        item.detail.clone(),
                    ),
                    ExecStatus::Ok(note) => ("✓".into(), Style::new().fg(OK), note.clone()),
                    ExecStatus::Failed(message) => {
                        ("✗".into(), Style::new().fg(ERR), message.clone())
                    }
                    ExecStatus::Skipped(message) => {
                        ("⊘".into(), Style::new().fg(WARN), message.clone())
                    }
                };
                let mut spans = vec![
                    Span::styled(format!(" {mark} "), style),
                    Span::raw(item.label.clone()),
                ];
                if !note.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", first_line(&note)),
                        style.add_modifier(Modifier::DIM),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>();
        let visible = steps_area.height.saturating_sub(2);
        let active = stage
            .items
            .iter()
            .position(|item| matches!(item.status, ExecStatus::Running))
            .unwrap_or(done.saturating_sub(1));
        let offset = if stage.running {
            list_offset(stage.items.len(), visible, active)
        } else {
            (stage.scroll as usize).min(stage.items.len().saturating_sub(visible as usize))
        };
        let list = List::new(items).block(bordered(" Tasks ", true));
        let mut state = ListState::default().with_offset(offset);
        frame.render_stateful_widget(list, steps_area, &mut state);

        if let Some(report) = &stage.report {
            let mut lines = Vec::new();
            if report.failures.is_empty() {
                lines.push(Line::styled("✓ All done", Style::new().fg(OK).bold()));
                lines.push(Line::from(format!(
                    "{} installed, nothing failed.",
                    report.installed.len()
                )));
            } else {
                let failed = report.failures.len();
                lines.push(Line::styled(
                    if failed == 1 {
                        "✗ Done, but one task failed".to_owned()
                    } else {
                        format!("✗ Done, but {failed} tasks failed")
                    },
                    Style::new().fg(ERR).bold(),
                ));
                lines.push(Line::from(format!(
                    "{} installed · {failed} failed — details above.",
                    report.installed.len()
                )));
            }
            for action in self.next_actions(report).iter().take(2) {
                lines.push(Line::styled(format!("next  {action}"), Style::new().dim()));
            }
            lines.push(Line::from(vec![
                Span::raw("Press "),
                Span::styled("Enter", Style::new().fg(ACCENT).bold()),
                Span::raw(" to finish."),
            ]));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .block(bordered(" Done ", true).padding(Padding::horizontal(1))),
                summary_area,
            );
        }
    }

    fn next_actions(&self, report: &crate::InstallReport) -> Vec<String> {
        let mut actions = Vec::new();
        for resource in self.selection() {
            let installed = report.installed.contains(&resource.id)
                || (resource.kind == crate::ResourceKind::Skill
                    && report.installed.iter().any(|target| target == "skills"));
            if installed && !actions.contains(&resource.next_action) {
                actions.push(resource.next_action.clone());
            }
        }
        actions
    }

    fn resource_item(&self, index: usize) -> ListItem<'_> {
        let resource = &self.model.resources[index];
        if self.model.installed[index] {
            return ListItem::new(Line::from(vec![
                Span::styled(" ✓ ", Style::new().fg(OK)),
                Span::styled(resource.label.as_str(), Style::new().dim()),
                Span::styled(" — installed", Style::new().dim()),
            ]));
        }
        let (mark, mark_style) = mark_for(self.selected[index]);
        ListItem::new(Line::from(vec![
            Span::styled(format!(" {mark} "), mark_style),
            Span::raw(resource.label.as_str()),
            Span::styled(format!(" — {}", resource.description), Style::new().dim()),
        ]))
    }
}

fn mark_for(selected: bool) -> (&'static str, Style) {
    if selected {
        (SELECTED_MARK, Style::new().fg(OK))
    } else {
        (UNSELECTED_MARK, Style::new().dim())
    }
}

fn runtime_name(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Pi => "Pi",
        Runtime::Herdr => "Herdr",
    }
}

fn runtime_blurb(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Pi => "Pi is the coding agent the packages plug into. Installed with npm.",
        Runtime::Herdr => {
            "Herdr is a terminal multiplexer for coding agents. The plugins run inside it."
        }
    }
}

fn bordered(title: &str, focused: bool) -> Block<'_> {
    let border_style = if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().dim()
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title.to_owned())
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new().bg(Color::DarkGray)
    } else {
        Style::new()
    }
}

fn list_title(name: &str, selected: usize, installed: usize, total: usize) -> String {
    if installed == 0 {
        format!(" {name} · {selected}/{total} selected ")
    } else if installed == total {
        format!(" {name} · all {total} installed ")
    } else {
        format!(
            " {name} · {installed} installed · {selected}/{} selected ",
            total - installed
        )
    }
}

fn list_offset(len: usize, visible: u16, cursor: usize) -> usize {
    let visible = visible.max(1) as usize;
    if cursor < visible || len <= visible {
        0
    } else {
        (cursor + 1 - visible).min(len - visible)
    }
}

fn centered(area: Rect, width: u16) -> Rect {
    let width = width.min(area.width);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y,
        width,
        height: area.height,
    }
}

fn count_kind(resources: &[Resource], kind: crate::ResourceKind) -> usize {
    resources
        .iter()
        .filter(|resource| resource.kind == kind)
        .count()
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}
