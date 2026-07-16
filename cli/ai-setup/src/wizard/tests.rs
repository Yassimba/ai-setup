use super::state::*;
use crate::settings::{
    KeyCommand, SettingChange, SettingSpec, SettingState, SettingsPaths, ZedKeybinding,
};
use crate::{Platform, PrerequisiteStatus, Resource, ResourceKind};
use pretty_assertions::assert_eq;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use serde_json::json;

fn resource(kind: ResourceKind, group: &str, label: &str) -> Resource {
    Resource {
        id: format!("{group}:{label}"),
        kind,
        group: group.into(),
        label: label.into(),
        description: "described".into(),
        install_target: label.into(),
        next_action: "next".into(),
        dependencies: Vec::new(),
    }
}

fn catalog() -> Vec<Resource> {
    vec![
        resource(ResourceKind::PiPackage, "Pi packages", "subagents"),
        resource(ResourceKind::PiPackage, "Pi packages", "themes"),
        resource(ResourceKind::HerdrPlugin, "Herdr plugins", "reviewr"),
        resource(ResourceKind::Skill, "Coding", "tdd"),
        resource(ResourceKind::Skill, "Coding", "refactor"),
        resource(ResourceKind::Skill, "Diagrams", "mermaid"),
    ]
}

fn test_settings() -> Vec<SettingSpec> {
    vec![
        SettingSpec {
            id: "herdr-key:reviewr".into(),
            group: "Herdr keybinds".into(),
            label: "Reviewr sidebar toggle".into(),
            description: "described".into(),
            related_resource: Some("Herdr plugins:reviewr".into()),
            change: SettingChange::HerdrKeyCommands(vec![KeyCommand {
                key: "prefix+r".into(),
                kind: "plugin_action".into(),
                command: "yassimba.reviewr.toggle".into(),
                description: None,
            }]),
        },
        SettingSpec {
            id: "zed:zoomed-padding".into(),
            group: "Zed".into(),
            label: "Zoomed panes edge-to-edge".into(),
            description: "described".into(),
            related_resource: None,
            change: SettingChange::ZedValue {
                key: "zoomed_padding".into(),
                value: json!(false),
            },
        },
        SettingSpec {
            id: "zed:reviewr-history-keys".into(),
            group: "Zed".into(),
            label: "⌘ arrows step Reviewr history".into(),
            description: "described".into(),
            related_resource: Some("Herdr plugins:reviewr".into()),
            change: SettingChange::ZedKeymap {
                context: "Terminal".into(),
                bindings: vec![ZedKeybinding {
                    key: "cmd-left".into(),
                    action: json!(["terminal::SendText", "x"]),
                }],
            },
        },
    ]
}

fn ready() -> PrerequisiteStatus {
    PrerequisiteStatus {
        pi: true,
        herdr: true,
        npm: true,
        node: crate::NodeStatus::Supported,
    }
}

fn model(status: PrerequisiteStatus) -> Model {
    let settings = test_settings();
    Model {
        resources: catalog(),
        installed: vec![false; catalog().len()],
        setting_states: vec![SettingState::NotApplied; settings.len()],
        settings,
        zed_present: false,
        settings_paths: SettingsPaths {
            herdr_config: "/tmp/herdr-config.toml".into(),
            zed_settings: "/tmp/zed-settings.json".into(),
            zed_keymap: "/tmp/zed-keymap.json".into(),
        },
        status,
        platform: Platform::Unix,
        dry_run: false,
    }
}

fn wizard() -> Wizard {
    Wizard::new(model(ready()))
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(wizard: &mut Wizard, codes: &[KeyCode]) -> Option<Action> {
    let mut action = None;
    for &code in codes {
        action = wizard.handle_key(key(code));
    }
    action
}

fn stage_titles(wizard: &Wizard) -> Vec<String> {
    wizard.stages.iter().map(|s| s.title().to_owned()).collect()
}

#[test]
fn stages_run_welcome_herdr_pi_skills_settings_review_install() {
    assert_eq!(
        stage_titles(&wizard()),
        ["Welcome", "Herdr", "Pi", "Skills", "Settings", "Review", "Install"]
    );
}

#[test]
fn kinds_without_resources_get_no_stage() {
    let mut skills_only = model(ready());
    skills_only
        .resources
        .retain(|r| r.kind == ResourceKind::Skill);
    skills_only.settings.clear();
    skills_only.setting_states.clear();
    let wizard = Wizard::new(skills_only);
    assert_eq!(
        stage_titles(&wizard),
        ["Welcome", "Skills", "Review", "Install"]
    );
}

#[test]
fn welcome_offers_missing_runtimes_for_quick_install() {
    let mut status = ready();
    status.pi = false;
    let mut wizard = Wizard::new(model(status));
    // Pi is missing, so it is pre-selected and the cursor starts on it.
    assert!(wizard.runtime_selected(crate::Runtime::Pi));
    let Stage::Welcome(stage) = &wizard.stages[0] else {
        panic!("expected the welcome stage");
    };
    assert_eq!(stage.rows.len(), 2, "herdr and pi are both listed");
    assert_eq!(
        stage.cursor, 1,
        "cursor starts on the first missing runtime"
    );
    // Space right on the welcome screen unchecks it, space again re-checks.
    press(&mut wizard, &[KeyCode::Char(' ')]);
    assert!(!wizard.runtime_selected(crate::Runtime::Pi));
    press(&mut wizard, &[KeyCode::Char(' ')]);
    assert!(wizard.runtime_selected(crate::Runtime::Pi));
    // With nothing else selected, confirming installs just the runtime.
    let action = press(&mut wizard, &[KeyCode::Enter; 6]);
    assert!(matches!(action, Some(Action::StartInstall)));
    let job = wizard.begin_install().unwrap();
    assert_eq!(job.plan.prerequisites.len(), 1);
    assert!(job.plan.prerequisites[0]
        .action
        .display()
        .contains("pi-coding-agent"));
}

#[test]
fn welcome_runtime_rows_are_clickable() {
    let mut status = ready();
    status.pi = false;
    let mut wizard = Wizard::new(model(status));
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| wizard.draw(frame)).unwrap();
    let (area, _) = wizard
        .hits
        .primary_list
        .expect("welcome has a runtime list");
    // Second row: pi (herdr is installed).
    wizard.handle_click(area.x + 2, area.y + 2);
    assert!(!wizard.runtime_selected(crate::Runtime::Pi));
    wizard.handle_click(area.x + 2, area.y + 2);
    assert!(wizard.runtime_selected(crate::Runtime::Pi));
}

#[test]
fn missing_runtimes_are_preselected_and_toggleable() {
    let mut status = ready();
    status.herdr = false;
    let mut wizard = Wizard::new(model(status));
    assert!(wizard.runtime_selected(crate::Runtime::Herdr));
    // The welcome cursor starts on the missing runtime; space unchecks it.
    press(&mut wizard, &[KeyCode::Char(' ')]);
    assert!(!wizard.runtime_selected(crate::Runtime::Herdr));
}

#[test]
fn installed_runtime_rows_are_not_toggleable() {
    let mut wizard = wizard();
    press(&mut wizard, &[KeyCode::Char(' '), KeyCode::Char('a')]);
    assert!(wizard.selected_runtimes().is_empty());
    assert!(wizard.selection().is_empty());
}

#[test]
fn installed_resources_are_shown_but_not_selectable() {
    let mut with_reviewr = model(ready());
    with_reviewr.installed[2] = true; // the reviewr plugin
    let mut wizard = Wizard::new(with_reviewr);
    // Space on the installed row does nothing; select-all skips it too.
    press(&mut wizard, &[KeyCode::Enter, KeyCode::Char(' ')]);
    assert!(wizard.selection().is_empty());
    press(&mut wizard, &[KeyCode::Char('a')]);
    assert!(wizard.selection().is_empty());
    // Skills: 'A' selects everything except an installed skill.
    let mut with_tdd = model(ready());
    with_tdd.installed[3] = true; // the tdd skill
    let mut wizard = Wizard::new(with_tdd);
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Char('A'),
        ],
    );
    assert_eq!(wizard.selected[3..], [false, true, true]);
}

#[test]
fn an_installed_plugin_prechecks_its_keybind() {
    let mut with_reviewr = model(ready());
    with_reviewr.installed[2] = true; // reviewr is present, its keybind is not
    let mut wizard = Wizard::new(with_reviewr);
    press(&mut wizard, &[KeyCode::Enter; 4]); // walk to settings
    assert_eq!(
        wizard
            .selected_settings()
            .iter()
            .map(|spec| spec.id.as_str())
            .collect::<Vec<_>>(),
        ["herdr-key:reviewr"]
    );
}

#[test]
fn skipping_herdr_and_pi_hides_their_stages_too() {
    let mut status = ready();
    status.herdr = false;
    status.pi = false;
    let mut wizard = Wizard::new(model(status));
    // Cursor starts on Herdr (the first missing tool); uncheck it and Pi.
    press(
        &mut wizard,
        &[KeyCode::Char(' '), KeyCode::Down, KeyCode::Char(' ')],
    );
    let visible = wizard
        .visible_stages()
        .into_iter()
        .map(|index| wizard.stages[index].title().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        visible,
        ["Welcome", "Skills", "Settings", "Review", "Install"]
    );
    // Enter from Welcome lands directly on Skills.
    press(&mut wizard, &[KeyCode::Enter]);
    assert!(matches!(
        wizard.stages[wizard.stage_index],
        Stage::Skills(_)
    ));
    // Herdr plugin and Pi package picks would not survive into the plan.
    wizard.selected[0] = true; // a pi package
    wizard.selected[2] = true; // the herdr plugin
    assert!(wizard.selection().is_empty());
}

#[test]
fn space_toggles_and_enter_reaches_review_then_install() {
    let mut wizard = wizard();
    let action = press(
        &mut wizard,
        &[
            KeyCode::Enter,     // welcome → herdr
            KeyCode::Down,      // onto reviewr
            KeyCode::Char(' '), // pick reviewr
            KeyCode::Enter,     // pi
            KeyCode::Enter,     // skills
            KeyCode::Enter,     // settings
            KeyCode::Enter,     // review
            KeyCode::Enter,     // confirm → install
        ],
    );
    assert!(matches!(action, Some(Action::StartInstall)));
    assert_eq!(
        wizard
            .selection()
            .iter()
            .map(|resource| resource.label.as_str())
            .collect::<Vec<_>>(),
        ["reviewr"]
    );
    assert!(
        wizard.install_running() || matches!(wizard.stages[wizard.stage_index], Stage::Install(_))
    );
}

#[test]
fn toggle_all_selects_then_clears_the_whole_pick_stage() {
    let mut wizard = wizard();
    press(&mut wizard, &[KeyCode::Enter, KeyCode::Char('a')]);
    assert_eq!(
        wizard.selection().len(),
        1,
        "herdr stage has one plugin; installed runtime is skipped"
    );
    press(&mut wizard, &[KeyCode::Char('a')]);
    assert!(wizard.selection().is_empty());
}

#[test]
fn welcome_toggle_all_flips_every_missing_runtime() {
    let mut status = ready();
    status.herdr = false;
    status.pi = false;
    let mut wizard = Wizard::new(model(status));
    // Both missing runtimes start on; 'a' clears them, 'a' restores them.
    press(&mut wizard, &[KeyCode::Char('a')]);
    assert!(wizard.selected_runtimes().is_empty());
    press(&mut wizard, &[KeyCode::Char('a')]);
    assert_eq!(wizard.selected_runtimes().len(), 2);
}

#[test]
fn space_on_a_category_toggles_only_that_category() {
    let mut wizard = wizard();
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Char(' '),
        ],
    );
    assert_eq!(wizard.selected[3..], [true, true, false]);
}

#[test]
fn capital_a_toggles_every_skill() {
    let mut wizard = wizard();
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Char('A'),
        ],
    );
    assert_eq!(wizard.selected[3..], [true, true, true]);
}

#[test]
fn skill_pane_navigates_and_toggles_single_skills() {
    let mut wizard = wizard();
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Down,
            KeyCode::Char(' '),
        ],
    );
    assert_eq!(wizard.selected[3..], [false, true, false]);
}

#[test]
fn settings_precheck_follows_the_related_plugin() {
    let mut wizard = wizard();
    // Select reviewr, then walk into the settings stage.
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
        ],
    );
    assert_eq!(
        wizard
            .selected_settings()
            .iter()
            .map(|spec| spec.id.as_str())
            .collect::<Vec<_>>(),
        ["herdr-key:reviewr"],
        "keybind pre-checks with its plugin; both Zed settings stay off \
         without a Zed install — even the plugin-related keymap one"
    );
}

#[test]
fn a_plugin_related_zed_setting_prechecks_only_with_zed_present() {
    let mut zed_model = model(ready());
    zed_model.zed_present = true;
    let mut wizard = Wizard::new(zed_model);
    // Select reviewr, then walk into the settings stage.
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
        ],
    );
    assert_eq!(
        wizard
            .selected_settings()
            .iter()
            .map(|spec| spec.id.as_str())
            .collect::<Vec<_>>(),
        [
            "herdr-key:reviewr",
            "zed:zoomed-padding",
            "zed:reviewr-history-keys",
        ],
        "with Zed present the reviewr keymap pre-checks alongside its plugin"
    );
}

#[test]
fn zed_settings_precheck_when_zed_is_present() {
    let mut zed_model = model(ready());
    zed_model.zed_present = true;
    let mut wizard = Wizard::new(zed_model);
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
        ],
    );
    assert_eq!(
        wizard
            .selected_settings()
            .iter()
            .map(|spec| spec.id.as_str())
            .collect::<Vec<_>>(),
        ["zed:zoomed-padding"]
    );
}

#[test]
fn a_user_uncheck_survives_the_precheck() {
    let mut wizard = wizard();
    // Walk to settings with reviewr selected, uncheck the keybind, go back
    // to review-adjacent stage and return: it must stay off.
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Char(' '), // uncheck the pre-checked keybind
            KeyCode::Backspace,
            KeyCode::Enter, // re-enter settings; precheck runs again
        ],
    );
    assert!(wizard.selected_settings().is_empty());
}

#[test]
fn applied_settings_cannot_be_selected() {
    let mut applied = model(ready());
    applied.setting_states = vec![SettingState::Applied; applied.settings.len()];
    let mut wizard = Wizard::new(applied);
    press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Char(' '),
            KeyCode::Char('a'),
        ],
    );
    assert!(wizard.selected_settings().is_empty());
}

#[test]
fn q_cancels_from_any_stage() {
    let mut wizard = wizard();
    let action = press(&mut wizard, &[KeyCode::Enter, KeyCode::Char('q')]);
    assert!(matches!(
        action,
        Some(Action::Exit(WizardOutcome::Cancelled))
    ));
}

#[test]
fn empty_selection_confirms_as_nothing_selected() {
    let mut wizard = wizard();
    let action = press(&mut wizard, &[KeyCode::Enter; 6]);
    assert!(matches!(
        action,
        Some(Action::Exit(WizardOutcome::NothingSelected))
    ));
}

#[test]
fn unbuildable_plan_blocks_confirmation() {
    let status = PrerequisiteStatus {
        pi: false,
        herdr: true,
        npm: false,
        node: crate::NodeStatus::Supported,
    };
    // Pi runtime is pre-checked because it is missing, and npm is missing
    // too, so the plan cannot be built.
    let mut wizard = Wizard::new(model(status));
    let action = press(&mut wizard, &[KeyCode::Enter; 6]);
    assert!(action.is_none());
    assert!(matches!(
        wizard.stages[wizard.stage_index],
        Stage::Review { .. }
    ));
}

#[test]
fn dry_run_exits_with_the_plan_instead_of_installing() {
    let mut dry = model(ready());
    dry.dry_run = true;
    let mut wizard = Wizard::new(dry);
    let action = press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
        ],
    );
    let Some(Action::Exit(WizardOutcome::DryRun(plan, _))) = action else {
        panic!("expected a dry-run exit");
    };
    assert_eq!(plan.resources.len(), 1);
}

#[test]
fn install_events_drive_the_install_screen_to_completion() {
    let mut wizard = wizard();
    let action = press(
        &mut wizard,
        &[
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Enter,
        ],
    );
    assert!(matches!(action, Some(Action::StartInstall)));
    let job = wizard.begin_install().unwrap();
    assert_eq!(job.plan.resources.len(), 1);
    assert!(wizard.install_running());
    // Keys are ignored while the worker runs.
    assert!(press(&mut wizard, &[KeyCode::Char('q')]).is_none());

    wizard.handle_install_event(InstallEvent::Status(0, ExecStatus::Running));
    wizard.handle_install_event(InstallEvent::Status(0, ExecStatus::Ok("installed".into())));
    wizard.handle_install_event(InstallEvent::Done(crate::InstallReport {
        installed: vec!["Herdr plugins:reviewr".into()],
        failures: Vec::new(),
    }));
    assert!(!wizard.install_running());
    let action = press(&mut wizard, &[KeyCode::Enter]);
    let Some(Action::Exit(WizardOutcome::Installed(report))) = action else {
        panic!("expected an installed exit");
    };
    assert_eq!(report.installed, vec!["Herdr plugins:reviewr".to_owned()]);
}

#[test]
fn every_stage_renders_without_panicking() {
    let mut status = ready();
    status.herdr = false;
    let mut wizard = Wizard::new(model(status));
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    for _ in 0..6 {
        terminal.draw(|frame| wizard.draw(frame)).unwrap();
        press(&mut wizard, &[KeyCode::Enter]);
    }
    // Install stage with items and a mixed set of statuses.
    let _ = wizard.begin_install().unwrap();
    wizard.handle_install_event(InstallEvent::Status(0, ExecStatus::Running));
    wizard.tick();
    terminal.draw(|frame| wizard.draw(frame)).unwrap();
    wizard.handle_install_event(InstallEvent::Status(0, ExecStatus::Failed("boom".into())));
    wizard.handle_install_event(InstallEvent::Done(crate::InstallReport {
        installed: Vec::new(),
        failures: vec![crate::InstallFailure {
            target: "HERDR".into(),
            message: "boom".into(),
        }],
    }));
    terminal.draw(|frame| wizard.draw(frame)).unwrap();
}

#[test]
fn clicking_a_pick_row_toggles_it() {
    let mut wizard = wizard();
    press(&mut wizard, &[KeyCode::Enter]); // herdr stage
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| wizard.draw(frame)).unwrap();
    let (area, _) = wizard.hits.primary_list.expect("pick stage has a list");
    // First row: the reviewr plugin (runtimes live on the Welcome screen).
    wizard.handle_click(area.x + 2, area.y + 1);
    assert_eq!(
        wizard
            .selection()
            .iter()
            .map(|resource| resource.label.as_str())
            .collect::<Vec<_>>(),
        ["reviewr"]
    );
}

#[test]
fn clicking_the_sidebar_jumps_only_to_visited_stages() {
    let mut wizard = wizard();
    press(&mut wizard, &[KeyCode::Enter, KeyCode::Enter]); // visited up to Pi
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| wizard.draw(frame)).unwrap();
    let sidebar = wizard.hits.sidebar;
    // Row 0: Welcome (visited) — jumps.
    wizard.handle_click(sidebar.x + 2, sidebar.y + 1);
    assert_eq!(wizard.stage_index, 0);
    // Review (index 5) was never visited — stays put.
    wizard.handle_click(sidebar.x + 2, sidebar.y + 1 + 5);
    assert_eq!(wizard.stage_index, 0);
}
