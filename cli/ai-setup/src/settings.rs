//! Curated tool-settings toggles offered by the setup wizard: Herdr plugin
//! keybindings and Zed screen-real-estate tweaks. Edits are format
//! preserving — `toml_edit` for Herdr's config.toml, a span-splicing JSONC
//! editor for Zed's commented settings.json.

use crate::jsonc;
use anyhow::{Context, Result};
use serde_json::{json, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyCommand {
    pub key: String,
    pub kind: String,
    pub command: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZedKeybinding {
    pub key: String,
    pub action: Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingChange {
    /// Append `[[keys.command]]` bindings to Herdr's config unless a binding
    /// for the same command already exists.
    HerdrKeyCommands(Vec<KeyCommand>),
    /// Set (or subset-merge into) a top-level key in Zed's settings.json.
    ZedValue { key: String, value: Json },
    /// Bind keys in one `context` block of Zed's keymap.json by appending a
    /// block at the end (a later block wins in Zed). A key the user already
    /// bound anywhere in that context is left alone.
    ZedKeymap {
        context: String,
        bindings: Vec<ZedKeybinding>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingSpec {
    pub id: String,
    pub group: String,
    pub label: String,
    pub description: String,
    /// Catalog resource whose selection should pre-check this setting.
    pub related_resource: Option<String>,
    pub change: SettingChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingState {
    Applied,
    NotApplied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsPaths {
    pub herdr_config: PathBuf,
    pub zed_settings: PathBuf,
    pub zed_keymap: PathBuf,
}

impl SettingsPaths {
    pub fn detect() -> Result<Self> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        // Herdr checks XDG_CONFIG_HOME before its platform default on every
        // platform, Windows and macOS included; follow it or the wizard
        // edits a config.toml Herdr never reads.
        let herdr_base = match &xdg {
            Some(base) => base.clone(),
            None => native_config_dir()?,
        };
        // Zed honors XDG_CONFIG_HOME only on Linux; Windows is always
        // %APPDATA%\Zed and macOS is always ~/.config/zed.
        let zed_dir = if cfg!(windows) {
            dirs::config_dir()
                .context("config directory is unavailable")?
                .join("Zed")
        } else if cfg!(target_os = "macos") {
            home_config_dir()?.join("zed")
        } else {
            match &xdg {
                Some(base) => base.join("zed"),
                None => home_config_dir()?.join("zed"),
            }
        };
        Ok(Self {
            herdr_config: herdr_base.join("herdr").join("config.toml"),
            zed_settings: zed_dir.join("settings.json"),
            zed_keymap: zed_dir.join("keymap.json"),
        })
    }
}

fn native_config_dir() -> Result<PathBuf> {
    if cfg!(windows) {
        dirs::config_dir().context("config directory is unavailable")
    } else {
        home_config_dir()
    }
}

fn home_config_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("home directory is unavailable")?
        .join(".config"))
}

/// Pick the Herdr action/entrypoint id the running platform's plugin
/// manifest registers. The Windows manifests register `-win` twins of every
/// action (`toggle-win`, `back-win`, …) because the unix entrypoints shell
/// out through sh; Herdr rejects a unix id there with platform_unsupported.
fn herdr_action_id(unix_id: &str) -> String {
    herdr_action_id_for(unix_id, cfg!(windows))
}

fn herdr_action_id_for(unix_id: &str, windows: bool) -> String {
    if windows {
        format!("{unix_id}-win")
    } else {
        unix_id.to_owned()
    }
}

pub fn curated_settings() -> Vec<SettingSpec> {
    // Reviewr steps Pi's edit history on alt-arrow bytes. macOS users press
    // ⌘ chords that Zed's terminal would otherwise swallow; Windows and
    // Linux bind alt+arrows directly.
    let (history_label, history_description, history_keys) = if cfg!(target_os = "macos") {
        (
            "⌘←/⌘→ step Reviewr's edit history",
            "Zed's terminal has no kitty keyboard protocol, so ⌘ chords never reach herdr panes. \
             Translate ⌘←/⌘→ into the alt-arrow bytes Reviewr binds for stepping through Pi's \
             edits.",
            ["cmd-left", "cmd-right"],
        )
    } else {
        (
            "alt+←/→ step Reviewr's edit history",
            "Bind alt+←/→ in Zed's terminal to the alt-arrow bytes Reviewr binds for stepping \
             through Pi's edits.",
            ["alt-left", "alt-right"],
        )
    };
    vec![
        SettingSpec {
            id: "herdr-key:reviewr".into(),
            group: "Herdr keybinds".into(),
            label: "Reviewr sidebar toggle".into(),
            description: "Bind prefix+r to toggle the Reviewr code-review sidebar.".into(),
            related_resource: Some("herdr-plugin:yassimba.reviewr".into()),
            change: SettingChange::HerdrKeyCommands(vec![KeyCommand {
                key: "prefix+r".into(),
                kind: "plugin_action".into(),
                command: herdr_action_id("yassimba.reviewr.toggle"),
                description: Some("Reviewr: toggle sidebar".into()),
            }]),
        },
        SettingSpec {
            id: "herdr-key:jumplist".into(),
            group: "Herdr keybinds".into(),
            label: "Jumplist pane history".into(),
            description:
                "Bind ctrl+alt+left/right to walk back and forward through pane focus history."
                    .into(),
            related_resource: Some("herdr-plugin:yassin.jumplist".into()),
            change: SettingChange::HerdrKeyCommands(vec![
                KeyCommand {
                    key: "ctrl+alt+left".into(),
                    kind: "plugin_action".into(),
                    command: herdr_action_id("yassin.jumplist.back"),
                    description: Some("Focus: previous pane (history)".into()),
                },
                KeyCommand {
                    key: "ctrl+alt+right".into(),
                    kind: "plugin_action".into(),
                    command: herdr_action_id("yassin.jumplist.forward"),
                    description: Some("Focus: next pane (history)".into()),
                },
            ]),
        },
        SettingSpec {
            id: "herdr-key:project-switcher".into(),
            group: "Herdr keybinds".into(),
            label: "Project switcher overlay".into(),
            description: "Bind prefix+{ to open the fuzzy project switcher overlay.".into(),
            related_resource: Some("herdr-plugin:yassin.project-switcher".into()),
            change: SettingChange::HerdrKeyCommands(vec![KeyCommand {
                key: "prefix+{".into(),
                kind: "shell".into(),
                command: format!(
                    "herdr plugin pane open --plugin yassin.project-switcher --entrypoint {}",
                    herdr_action_id("switch")
                ),
                description: Some("Project: switch / open".into()),
            }]),
        },
        SettingSpec {
            id: "zed:reviewr-history-keys".into(),
            group: "Zed".into(),
            label: history_label.into(),
            description: history_description.into(),
            related_resource: Some("herdr-plugin:yassimba.reviewr".into()),
            change: SettingChange::ZedKeymap {
                context: "Terminal".into(),
                bindings: vec![
                    ZedKeybinding {
                        key: history_keys[0].into(),
                        action: json!(["terminal::SendText", "\u{1b}[1;3D"]),
                    },
                    ZedKeybinding {
                        key: history_keys[1].into(),
                        action: json!(["terminal::SendText", "\u{1b}[1;3C"]),
                    },
                ],
            },
        },
        SettingSpec {
            id: "zed:zoomed-padding".into(),
            group: "Zed".into(),
            label: "Zoomed panes edge-to-edge".into(),
            description:
                "Remove the gap Zed keeps around a zoomed pane, like a full-screen terminal.".into(),
            related_resource: None,
            change: SettingChange::ZedValue {
                key: "zoomed_padding".into(),
                value: json!(false),
            },
        },
        SettingSpec {
            id: "zed:zen-padding".into(),
            group: "Zed".into(),
            label: "Zen mode edge-to-edge".into(),
            description:
                "Let zen mode use the full editor width instead of a padded center column.".into(),
            related_resource: None,
            change: SettingChange::ZedValue {
                key: "centered_layout".into(),
                value: json!({"left_padding": 0, "right_padding": 0}),
            },
        },
    ]
}

impl SettingSpec {
    pub fn target_path<'a>(&self, paths: &'a SettingsPaths) -> &'a Path {
        match self.change {
            SettingChange::HerdrKeyCommands(_) => &paths.herdr_config,
            SettingChange::ZedValue { .. } => &paths.zed_settings,
            SettingChange::ZedKeymap { .. } => &paths.zed_keymap,
        }
    }

    /// Whether the change writes to a Zed config file — such a setting only
    /// makes sense on a machine that has Zed at all.
    pub fn requires_zed(&self) -> bool {
        matches!(
            self.change,
            SettingChange::ZedValue { .. } | SettingChange::ZedKeymap { .. }
        )
    }

    /// A short, review-screen friendly rendition of what gets written.
    pub fn change_summary(&self) -> Vec<String> {
        match &self.change {
            SettingChange::HerdrKeyCommands(commands) => commands
                .iter()
                .map(|command| format!("[[keys.command]] {} → {}", command.key, command.command))
                .collect(),
            SettingChange::ZedValue { key, value } => vec![format!("\"{key}\": {value}")],
            SettingChange::ZedKeymap { context, bindings } => bindings
                .iter()
                .map(|binding| format!("{context}: \"{}\" → {}", binding.key, binding.action))
                .collect(),
        }
    }
}

pub fn setting_state(spec: &SettingSpec, paths: &SettingsPaths) -> SettingState {
    let content = match fs::read_to_string(spec.target_path(paths)) {
        Ok(content) => content,
        Err(_) => return SettingState::NotApplied,
    };
    let applied = match &spec.change {
        SettingChange::HerdrKeyCommands(commands) => content
            .parse::<DocumentMut>()
            .map(|document| {
                commands
                    .iter()
                    .all(|command| herdr_has_binding(&document, &command.command))
            })
            .unwrap_or(false),
        SettingChange::ZedValue { key, value } => jsonc::get(&content, key)
            .map(|current| json_subset(value, &current))
            .unwrap_or(false),
        SettingChange::ZedKeymap { context, bindings } => jsonc::parse_document(&content)
            .map(|document| {
                bindings
                    .iter()
                    .all(|binding| zed_keymap_binds(&document, context, &binding.key))
            })
            .unwrap_or(false),
    };
    if applied {
        SettingState::Applied
    } else {
        SettingState::NotApplied
    }
}

/// Apply the setting; returns false when the file already had it.
pub fn apply_setting(spec: &SettingSpec, paths: &SettingsPaths) -> Result<bool> {
    let path = spec.target_path(paths);
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match &spec.change {
            SettingChange::HerdrKeyCommands(_) => String::new(),
            SettingChange::ZedValue { .. } => "{}\n".into(),
            SettingChange::ZedKeymap { .. } => "[]\n".into(),
        },
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()))
        }
    };
    let updated = match &spec.change {
        SettingChange::HerdrKeyCommands(commands) => apply_herdr_bindings(&existing, commands)?,
        SettingChange::ZedValue { key, value } => apply_zed_value(&existing, key, value)?,
        SettingChange::ZedKeymap { context, bindings } => {
            apply_zed_keymap(&existing, context, bindings)?
        }
    };
    let Some(updated) = updated else {
        return Ok(false);
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    fs::write(path, updated).with_context(|| format!("could not write {}", path.display()))?;
    Ok(true)
}

fn herdr_has_binding(document: &DocumentMut, command: &str) -> bool {
    document
        .get("keys")
        .and_then(Item::as_table)
        .and_then(|keys| keys.get("command"))
        .and_then(Item::as_array_of_tables)
        .is_some_and(|bindings| {
            bindings
                .iter()
                .any(|binding| binding.get("command").and_then(Item::as_str) == Some(command))
        })
}

pub fn apply_herdr_bindings(content: &str, commands: &[KeyCommand]) -> Result<Option<String>> {
    let mut document: DocumentMut = content
        .parse()
        .context("Herdr config.toml could not be parsed")?;
    let missing = commands
        .iter()
        .filter(|command| !herdr_has_binding(&document, &command.command))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(None);
    }
    let keys = document
        .entry("keys")
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_mut()
        .context("Herdr config has a non-table [keys] entry")?;
    let bindings = keys
        .entry("command")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("Herdr config has a non-array keys.command entry")?;
    for command in missing {
        let mut binding = Table::new();
        binding["key"] = toml_edit::value(command.key);
        binding["type"] = toml_edit::value(command.kind);
        binding["command"] = toml_edit::value(command.command);
        if let Some(description) = command.description {
            binding["description"] = toml_edit::value(description);
        }
        bindings.push(binding);
    }
    Ok(Some(document.to_string()))
}

/// True when any block of the keymap with this `context` binds `key` — to
/// anything. A user's own binding for the key counts as applied, so setup
/// never overrides a deliberate choice.
fn zed_keymap_binds(document: &Json, context: &str, key: &str) -> bool {
    document.as_array().is_some_and(|blocks| {
        blocks.iter().any(|block| {
            block.get("context").and_then(Json::as_str) == Some(context)
                && block
                    .get("bindings")
                    .and_then(Json::as_object)
                    .is_some_and(|bindings| bindings.contains_key(key))
        })
    })
}

pub fn apply_zed_keymap(
    content: &str,
    context: &str,
    bindings: &[ZedKeybinding],
) -> Result<Option<String>> {
    let document = jsonc::parse_document(content).unwrap_or_else(|_| Json::Array(Vec::new()));
    let missing = bindings
        .iter()
        .filter(|binding| !zed_keymap_binds(&document, context, &binding.key))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(None);
    }
    // Rendered by hand: serde would sort the keys ("bindings" before
    // "context") and the block should read the way Zed's docs write it.
    let mut block = String::from("{\n");
    block.push_str(&format!(
        "  \"context\": {},\n",
        Json::String(context.into())
    ));
    block.push_str("  \"bindings\": {\n");
    for binding in missing {
        block.push_str(&format!(
            "    {}: {},\n",
            Json::String(binding.key.clone()),
            binding.action
        ));
    }
    block.push_str("  }\n}");
    jsonc::push_root_array_item(content, &block).map(Some)
}

pub fn apply_zed_value(content: &str, key: &str, value: &Json) -> Result<Option<String>> {
    let current = jsonc::get(content, key);
    if current
        .as_ref()
        .is_some_and(|current| json_subset(value, current))
    {
        return Ok(None);
    }
    // Object values merge over what is already there so unrelated fields of
    // the same object survive; scalars replace.
    let merged = match (value, &current) {
        (Json::Object(wanted), Some(Json::Object(existing))) => {
            let mut merged = existing.clone();
            for (name, item) in wanted {
                merged.insert(name.clone(), item.clone());
            }
            Json::Object(merged)
        }
        _ => value.clone(),
    };
    jsonc::set(content, key, &merged).map(Some)
}

/// True when every field of `expected` is present in `actual` (recursively);
/// scalars compare by equality.
fn json_subset(expected: &Json, actual: &Json) -> bool {
    match (expected, actual) {
        (Json::Object(expected), Json::Object(actual)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|actual| json_subset(value, actual))
        }),
        _ => expected == actual || numbers_equal(expected, actual),
    }
}

fn numbers_equal(expected: &Json, actual: &Json) -> bool {
    match (expected.as_f64(), actual.as_f64()) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn reviewr_binding() -> Vec<KeyCommand> {
        vec![KeyCommand {
            key: "prefix+r".into(),
            kind: "plugin_action".into(),
            command: "yassimba.reviewr.toggle".into(),
            description: Some("Reviewr: toggle sidebar".into()),
        }]
    }

    #[test]
    fn herdr_binding_is_appended_once() {
        let base = "[theme]\nname = \"catppuccin\"\n\n[keys]\nprefix = \"alt+space\"\n";
        let updated = apply_herdr_bindings(base, &reviewr_binding())
            .unwrap()
            .expect("first apply changes the file");
        assert!(updated.contains("[[keys.command]]"));
        assert!(updated.contains("command = \"yassimba.reviewr.toggle\""));
        assert!(updated.contains("name = \"catppuccin\""));
        assert_eq!(
            apply_herdr_bindings(&updated, &reviewr_binding()).unwrap(),
            None
        );
    }

    #[test]
    fn herdr_binding_detection_matches_on_command_not_key() {
        let base = concat!(
            "[keys]\n",
            "[[keys.command]]\n",
            "key = \"prefix+x\"\n",
            "type = \"plugin_action\"\n",
            "command = \"yassimba.reviewr.toggle\"\n",
        );
        assert_eq!(
            apply_herdr_bindings(base, &reviewr_binding()).unwrap(),
            None
        );
    }

    #[test]
    fn herdr_bindings_create_the_keys_table_when_missing() {
        let updated = apply_herdr_bindings("", &reviewr_binding())
            .unwrap()
            .expect("apply changes the file");
        let document: DocumentMut = updated.parse().unwrap();
        assert!(herdr_has_binding(&document, "yassimba.reviewr.toggle"));
    }

    #[test]
    fn zed_value_preserves_comments_and_merges_objects() {
        let base = "// keep me\n{\n  \"centered_layout\": {\n    \"enabled\": true,\n  },\n}\n";
        let updated = apply_zed_value(
            base,
            "centered_layout",
            &json!({"left_padding": 0, "right_padding": 0}),
        )
        .unwrap()
        .expect("apply changes the file");
        assert!(updated.contains("// keep me"));
        assert_eq!(
            jsonc::get(&updated, "centered_layout"),
            Some(json!({"enabled": true, "left_padding": 0, "right_padding": 0}))
        );
    }

    #[test]
    fn zed_value_is_a_noop_when_already_a_subset() {
        let base = "{\n  \"zoomed_padding\": false,\n}\n";
        assert_eq!(
            apply_zed_value(base, "zoomed_padding", &json!(false)).unwrap(),
            None
        );
        let padded = "{\n  \"centered_layout\": { \"left_padding\": 0.0, \"right_padding\": 0, \"enabled\": true },\n}\n";
        assert_eq!(
            apply_zed_value(
                padded,
                "centered_layout",
                &json!({"left_padding": 0, "right_padding": 0})
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn herdr_action_ids_pick_the_windows_twin_only_on_windows() {
        assert_eq!(
            herdr_action_id_for("yassimba.reviewr.toggle", true),
            "yassimba.reviewr.toggle-win"
        );
        assert_eq!(
            herdr_action_id_for("yassimba.reviewr.toggle", false),
            "yassimba.reviewr.toggle"
        );
    }

    #[test]
    fn curated_settings_bind_platform_native_herdr_action_ids() {
        let commands = curated_settings()
            .iter()
            .filter_map(|spec| match &spec.change {
                SettingChange::HerdrKeyCommands(commands) => Some(commands.clone()),
                _ => None,
            })
            .flatten()
            .map(|command| command.command)
            .collect::<Vec<_>>();
        let expected: Vec<String> = if cfg!(windows) {
            vec![
                "yassimba.reviewr.toggle-win".into(),
                "yassin.jumplist.back-win".into(),
                "yassin.jumplist.forward-win".into(),
                "herdr plugin pane open --plugin yassin.project-switcher --entrypoint switch-win"
                    .into(),
            ]
        } else {
            vec![
                "yassimba.reviewr.toggle".into(),
                "yassin.jumplist.back".into(),
                "yassin.jumplist.forward".into(),
                "herdr plugin pane open --plugin yassin.project-switcher --entrypoint switch"
                    .into(),
            ]
        };
        assert_eq!(commands, expected);
    }

    #[test]
    fn curated_settings_relate_to_catalog_plugin_ids() {
        let related = curated_settings()
            .into_iter()
            .filter_map(|spec| spec.related_resource)
            .collect::<Vec<_>>();
        assert_eq!(
            related,
            vec![
                "herdr-plugin:yassimba.reviewr",
                "herdr-plugin:yassin.jumplist",
                "herdr-plugin:yassin.project-switcher",
                "herdr-plugin:yassimba.reviewr",
            ]
        );
    }

    fn history_keys() -> Vec<ZedKeybinding> {
        vec![
            ZedKeybinding {
                key: "cmd-left".into(),
                action: json!(["terminal::SendText", "\u{1b}[1;3D"]),
            },
            ZedKeybinding {
                key: "cmd-right".into(),
                action: json!(["terminal::SendText", "\u{1b}[1;3C"]),
            },
        ]
    }

    #[test]
    fn zed_keymap_appends_a_terminal_block_and_preserves_comments() {
        let base = "// keep me\n[\n  {\n    \"context\": \"Editor\",\n    \"bindings\": {\n      \"cmd-left\": \"pane::GoBack\"\n    }\n  }\n]\n";
        let updated = apply_zed_keymap(base, "Terminal", &history_keys())
            .unwrap()
            .expect("first apply changes the file");
        assert!(updated.contains("// keep me"));
        assert!(
            updated.contains("\\u001b[1;3D"),
            "the escape byte is written as a JSON escape, not raw: {updated}"
        );
        let document = jsonc::parse_document(&updated).unwrap();
        assert!(zed_keymap_binds(&document, "Terminal", "cmd-left"));
        assert!(zed_keymap_binds(&document, "Terminal", "cmd-right"));
        // The Editor block's own cmd-left is untouched and distinct.
        assert_eq!(
            document[0]["bindings"]["cmd-left"],
            json!("pane::GoBack"),
            "existing contexts are never edited"
        );
        assert_eq!(
            apply_zed_keymap(&updated, "Terminal", &history_keys()).unwrap(),
            None,
            "second apply is a no-op"
        );
    }

    #[test]
    fn zed_keymap_respects_a_users_own_binding_for_one_of_the_keys() {
        let base = "[\n  {\n    \"context\": \"Terminal\",\n    \"bindings\": {\n      \"cmd-left\": \"something::Custom\"\n    }\n  }\n]\n";
        let updated = apply_zed_keymap(base, "Terminal", &history_keys())
            .unwrap()
            .expect("the unbound key still gets added");
        let document = jsonc::parse_document(&updated).unwrap();
        assert_eq!(
            document[0]["bindings"]["cmd-left"],
            json!("something::Custom"),
            "the user's own cmd-left binding survives"
        );
        assert!(zed_keymap_binds(&document, "Terminal", "cmd-right"));
    }

    #[test]
    fn zed_keymap_builds_the_file_from_nothing() {
        let updated = apply_zed_keymap("[]\n", "Terminal", &history_keys())
            .unwrap()
            .expect("apply changes the file");
        let document = jsonc::parse_document(&updated).unwrap();
        assert!(zed_keymap_binds(&document, "Terminal", "cmd-left"));
    }
}
