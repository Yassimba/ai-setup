//! Command-line flags and the shared plugin configuration boundary.
//!
//! See `specs/tui.md` and `specs/herdr-host.md`. Flags override defaults; the positional
//! argument (if any) is the repo path, else the current directory.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Resolved runtime configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub repo: PathBuf,
    pub poll: Duration,
    pub base: Option<String>,
    pub theme: Option<String>,
    /// `Some(false)` when `--wrap off` is passed; `None` keeps the default (wrap on).
    pub wrap: Option<bool>,
    /// Deep Review mode: the collaboration target key this instance exclusively serves.
    pub deep: Option<String>,
}

impl Config {
    /// Parse `args` (the process arguments *after* argv\[0\]).
    ///
    /// Recognises `--poll <ms>` (min 200, default 2000), `--base <ref>`,
    /// `--theme <name>`, and `--wrap on|off`; the first non-flag token is the repo path.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let mut repo: Option<PathBuf> = None;
        let mut poll_ms: u64 = 2000;
        let mut base: Option<String> = None;
        let mut theme: Option<String> = None;
        let mut wrap: Option<bool> = None;
        let mut deep: Option<String> = None;
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--poll" => {
                    if let Some(v) = it.next() {
                        poll_ms = v.parse().unwrap_or(poll_ms);
                    }
                }
                "--base" => base = it.next(),
                "--theme" => theme = it.next(),
                "--wrap" => wrap = it.next().map(|v| v != "off"),
                "--deep" => deep = it.next(),
                other if !other.starts_with('-') => repo = Some(PathBuf::from(other)),
                _ => {}
            }
        }
        let repo =
            repo.or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."));
        Self { repo, poll: Duration::from_millis(poll_ms.max(200)), base, theme, wrap, deep }
    }

    /// Parse from the real process arguments.
    pub fn from_env() -> Self {
        Self::parse(std::env::args().skip(1))
    }
}

/// The built-in base-branch candidates for the `branch` scope, used when `config.toml`
/// sets no `base_branches` (`specs/review-model.md`).
pub const DEFAULT_BASE_BRANCHES: [&str; 4] = ["origin/main", "origin/master", "main", "master"];

const PLUGIN_CONFIG_KEYS: [&str; 9] = [
    "theme",
    "base_branches",
    "toggle_placement",
    "toggle_direction",
    "auto_open",
    "github_host",
    "gitlab_host",
    "switcher_roots",
    "deep_pi_model",
];

/// Where the toggle action opens the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TogglePlacement {
    Split,
    Overlay,
    Zoomed,
    Tab,
}

impl TogglePlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::Overlay => "overlay",
            Self::Zoomed => "zoomed",
            Self::Tab => "tab",
        }
    }
}

/// Direction for split placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleDirection {
    Right,
    Down,
}

impl ToggleDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

/// One validated snapshot of `$HERDR_PLUGIN_CONFIG_DIR/config.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConfig {
    theme: String,
    base_branches: Vec<String>,
    toggle_placement: TogglePlacement,
    toggle_direction: ToggleDirection,
    auto_open: bool,
    github_host: Option<String>,
    gitlab_host: Option<String>,
    switcher_roots: Vec<String>,
    deep_pi_model: Option<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT.to_owned(),
            base_branches: DEFAULT_BASE_BRANCHES.iter().map(|s| (*s).to_owned()).collect(),
            toggle_placement: TogglePlacement::Split,
            toggle_direction: ToggleDirection::Right,
            auto_open: true,
            github_host: None,
            gitlab_host: None,
            switcher_roots: Vec::new(),
            deep_pi_model: None,
        }
    }
}

impl PluginConfig {
    pub fn theme(&self) -> &str {
        &self.theme
    }

    pub fn base_branches(&self) -> &[String] {
        &self.base_branches
    }

    pub fn toggle_placement(&self) -> TogglePlacement {
        self.toggle_placement
    }

    pub fn toggle_direction(&self) -> ToggleDirection {
        self.toggle_direction
    }

    pub fn auto_open(&self) -> bool {
        self.auto_open
    }

    pub fn github_host(&self) -> Option<&str> {
        self.github_host.as_deref()
    }

    pub fn gitlab_host(&self) -> Option<&str> {
        self.gitlab_host.as_deref()
    }

    /// The project-switcher search roots (`~` expands at use). Empty means unset — the
    /// switcher then lists the current repo's siblings (`specs/tui.md#project-switcher`).
    pub fn switcher_roots(&self) -> &[String] {
        &self.switcher_roots
    }

    /// The model the Deep Review workspace pins its Pi to (`pi --model <value>`).
    /// `None` leaves Pi on its own default resolution.
    pub fn deep_pi_model(&self) -> Option<&str> {
        self.deep_pi_model.as_deref()
    }

    /// One normalized machine-readable snapshot; kept for the config test that pins every
    /// key's presence and default.
    #[cfg(test)]
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "theme": self.theme,
            "base_branches": self.base_branches,
            "toggle_placement": self.toggle_placement.as_str(),
            "toggle_direction": self.toggle_direction.as_str(),
            "auto_open": self.auto_open,
            "github_host": self.github_host,
            "gitlab_host": self.gitlab_host,
            "switcher_roots": self.switcher_roots,
            "deep_pi_model": self.deep_pi_model,
        })
    }
}

/// A whole-file configuration failure. It keeps the path in the value so every entry point can
/// show the same actionable diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConfigError {
    path: PathBuf,
    detail: String,
}

impl PluginConfigError {
    fn new(path: &Path, detail: impl Into<String>) -> Self {
        Self { path: path.to_owned(), detail: detail.into() }
    }
}

impl fmt::Display for PluginConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "config {}: {}", self.path.display(), self.detail)
    }
}

impl std::error::Error for PluginConfigError {}

/// Read one plugin config snapshot from the process environment. An unset config directory is
/// standalone mode and uses defaults; a configured directory always names `config.toml`.
pub fn plugin_config() -> Result<PluginConfig, PluginConfigError> {
    let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") else {
        return Ok(PluginConfig::default());
    };
    plugin_config_in(dir)
}

/// Read one plugin config snapshot from `<dir>/config.toml`.
pub fn plugin_config_in(dir: impl AsRef<Path>) -> Result<PluginConfig, PluginConfigError> {
    parse_plugin_config(&dir.as_ref().join("config.toml"))
}

fn parse_plugin_config(path: &Path) -> Result<PluginConfig, PluginConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(PluginConfig::default()),
        Err(error) => {
            return Err(PluginConfigError::new(path, format!("read failed: {error}")));
        }
    };
    let table: toml::Table = text.parse().map_err(|error: toml::de::Error| {
        PluginConfigError::new(path, format!("syntax error: {}", error.message()))
    })?;
    if let Some(key) = table.keys().find(|key| !PLUGIN_CONFIG_KEYS.contains(&key.as_str())) {
        return Err(PluginConfigError::new(
            path,
            format!("unknown key {key:?}; expected one of {}", PLUGIN_CONFIG_KEYS.join(", ")),
        ));
    }

    let mut config = PluginConfig::default();
    if let Some(value) = table.get("theme") {
        let theme = string_value(path, "theme", value, "a built-in theme name")?;
        if !crate::theme::is_known(theme) {
            return Err(PluginConfigError::new(
                path,
                format!("invalid value for `theme`: {theme:?}; expected a built-in theme name"),
            ));
        }
        theme.clone_into(&mut config.theme);
    }
    if let Some(value) = table.get("base_branches") {
        let Some(values) = value.as_array() else {
            return Err(value_error(
                path,
                "base_branches",
                "a non-empty array of non-empty strings",
            ));
        };
        if values.is_empty() {
            return Err(value_error(
                path,
                "base_branches",
                "a non-empty array of non-empty strings",
            ));
        }
        let mut branches = Vec::with_capacity(values.len());
        for value in values {
            let Some(branch) = value.as_str() else {
                return Err(value_error(
                    path,
                    "base_branches",
                    "a non-empty array of non-empty strings",
                ));
            };
            if !valid_ref_name(branch) {
                return Err(value_error(
                    path,
                    "base_branches",
                    "a non-empty array of Git ref names",
                ));
            }
            branches.push(branch.to_owned());
        }
        config.base_branches = branches;
    }
    if let Some(value) = table.get("toggle_placement") {
        config.toggle_placement = match string_value(
            path,
            "toggle_placement",
            value,
            "one of split, overlay, zoomed, tab",
        )? {
            "split" => TogglePlacement::Split,
            "overlay" => TogglePlacement::Overlay,
            "zoomed" => TogglePlacement::Zoomed,
            "tab" => TogglePlacement::Tab,
            _ => {
                return Err(value_error(
                    path,
                    "toggle_placement",
                    "one of split, overlay, zoomed, tab",
                ));
            }
        };
    }
    if let Some(value) = table.get("toggle_direction") {
        config.toggle_direction =
            match string_value(path, "toggle_direction", value, "one of right, down")? {
                "right" => ToggleDirection::Right,
                "down" => ToggleDirection::Down,
                _ => return Err(value_error(path, "toggle_direction", "one of right, down")),
            };
    }
    if let Some(value) = table.get("auto_open") {
        config.auto_open =
            value.as_bool().ok_or_else(|| value_error(path, "auto_open", "a boolean"))?;
    }
    if let Some(value) = table.get("deep_pi_model") {
        let model =
            string_value(path, "deep_pi_model", value, "a model pattern such as provider/id")?;
        if model.trim().is_empty() {
            return Err(value_error(path, "deep_pi_model", "a non-empty model pattern"));
        }
        config.deep_pi_model = Some(model.to_owned());
    }
    if let Some(value) = table.get("github_host") {
        let host = string_value(path, "github_host", value, "a bare hostname outside github.com")?;
        if !valid_enterprise_host(host) {
            return Err(value_error(
                path,
                "github_host",
                "a bare hostname outside the github.com and github.com-* namespace",
            ));
        }
        config.github_host = Some(host.to_ascii_lowercase());
    }
    if let Some(value) = table.get("gitlab_host") {
        let host = string_value(path, "gitlab_host", value, "a bare hostname outside gitlab.com")?;
        if !valid_selfhosted_gitlab_host(host) {
            return Err(value_error(
                path,
                "gitlab_host",
                "a bare hostname outside the gitlab.com and gitlab.com-* namespace",
            ));
        }
        config.gitlab_host = Some(host.to_ascii_lowercase());
    }
    if let Some(value) = table.get("switcher_roots") {
        let expected = "an array of non-empty directory paths";
        let Some(values) = value.as_array() else {
            return Err(value_error(path, "switcher_roots", expected));
        };
        let mut roots = Vec::with_capacity(values.len());
        for value in values {
            match value.as_str() {
                Some(root) if !root.is_empty() => roots.push(root.to_owned()),
                _ => return Err(value_error(path, "switcher_roots", expected)),
            }
        }
        config.switcher_roots = roots;
    }
    Ok(config)
}

fn string_value<'a>(
    path: &Path,
    key: &str,
    value: &'a toml::Value,
    expected: &str,
) -> Result<&'a str, PluginConfigError> {
    value.as_str().ok_or_else(|| value_error(path, key, expected))
}

fn value_error(path: &Path, key: &str, expected: &str) -> PluginConfigError {
    PluginConfigError::new(path, format!("invalid value for `{key}`; expected {expected}"))
}

/// `gitlab_host` shares the enterprise shape rules but guards the gitlab.com namespace instead.
fn valid_selfhosted_gitlab_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if lower == "gitlab.com" || lower.starts_with("gitlab.com-") {
        return false;
    }
    valid_host_shape(host)
}

fn valid_enterprise_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if lower == "github.com" || lower.starts_with("github.com-") {
        return false;
    }
    valid_host_shape(host)
}

fn valid_host_shape(host: &str) -> bool {
    if host.len() > 253 {
        return false;
    }
    let mut labels = host.split('.').peekable();
    if labels.peek().is_none() {
        return false;
    }
    labels.all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

/// Git's `check-ref-format --allow-onelevel` rules, used without spawning Git from the shared
/// configuration boundary. Base entries are names, not revision expressions.
fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && name != "@"
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("//")
        && !name.contains("..")
        && !name.contains("@{")
        && name
            .split('/')
            .all(|part| !part.starts_with('.') && part.strip_suffix(".lock").is_none())
        && name.bytes().all(|byte| {
            byte > b' '
                && byte != 0x7f
                && !matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}

#[cfg(test)]
mod tests {
    use super::{Config, PluginConfig, ToggleDirection, TogglePlacement};
    use std::time::Duration;

    fn parse(args: &[&str]) -> Config {
        Config::parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn defaults_when_no_args() {
        let c = parse(&[]);
        assert_eq!(c.poll, Duration::from_secs(2));
        assert_eq!(c.base, None);
    }

    #[test]
    fn flags_and_positional_repo() {
        let c = parse(&["--poll", "500", "--base", "origin/dev", "/tmp/work"]);
        assert_eq!(c.poll, Duration::from_millis(500));
        assert_eq!(c.base.as_deref(), Some("origin/dev"));
        assert_eq!(c.repo.to_str(), Some("/tmp/work"));
    }

    #[test]
    fn poll_has_a_floor() {
        assert_eq!(parse(&["--poll", "10"]).poll, Duration::from_millis(200));
        assert_eq!(parse(&["--poll", "garbage"]).poll, Duration::from_secs(2));
    }

    #[test]
    fn missing_file_uses_all_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(super::plugin_config_in(dir.path()).unwrap(), PluginConfig::default());
    }

    #[test]
    fn omitted_keys_keep_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "theme = \"gruvbox\"\n").unwrap();
        let config = super::plugin_config_in(dir.path()).unwrap();
        assert_eq!(config.theme(), "gruvbox");
        assert_eq!(config.base_branches(), PluginConfig::default().base_branches());
        assert_eq!(config.toggle_placement(), TogglePlacement::Split);
        assert_eq!(config.toggle_direction(), ToggleDirection::Right);
        assert!(config.auto_open());
        assert_eq!(config.github_host(), None);
        assert!(config.switcher_roots().is_empty());
    }

    #[test]
    fn reads_complete_valid_file_as_one_value() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            concat!(
                "theme = \"tokyo-night\"\n",
                "base_branches = [\"origin/dev\", \"main\"]\n",
                "toggle_placement = \"overlay\"\n",
                "toggle_direction = \"down\"\n",
                "auto_open = false\n",
                "github_host = \"GitHub.Example.COM\"\n",
                "switcher_roots = [\"~/Documents/projects\", \"/srv/work\"]\n",
            ),
        )
        .unwrap();
        let config = super::plugin_config_in(dir.path()).unwrap();
        assert_eq!(config.theme(), "tokyo-night");
        assert_eq!(config.base_branches(), ["origin/dev", "main"]);
        assert_eq!(config.toggle_placement(), TogglePlacement::Overlay);
        assert_eq!(config.toggle_direction(), ToggleDirection::Down);
        assert!(!config.auto_open());
        assert_eq!(config.github_host(), Some("github.example.com"));
        assert_eq!(config.switcher_roots(), ["~/Documents/projects", "/srv/work"]);
    }

    #[test]
    fn unknown_key_and_syntax_error_fail_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"gruvbox\"\npoll = 500\n").unwrap();
        let error = super::plugin_config_in(dir.path()).unwrap_err().to_string();
        assert!(error.contains(path.to_str().unwrap()));
        assert!(error.contains("unknown key \"poll\""));

        std::fs::write(&path, "theme = [\n").unwrap();
        assert!(
            super::plugin_config_in(dir.path()).unwrap_err().to_string().contains("syntax error")
        );
    }

    #[test]
    fn every_invalid_value_fails_instead_of_falling_back() {
        let cases = [
            ("theme = \"unknown\"\n", "`theme`"),
            ("base_branches = []\n", "`base_branches`"),
            ("base_branches = [\"\"]\n", "`base_branches`"),
            ("base_branches = [\"main^{commit}\"]\n", "`base_branches`"),
            ("base_branches = [\"feature branch\"]\n", "`base_branches`"),
            ("base_branches = [\"-main\"]\n", "`base_branches`"),
            ("base_branches = [\"main\", 1]\n", "`base_branches`"),
            ("toggle_placement = \"left\"\n", "`toggle_placement`"),
            ("toggle_direction = \"left\"\n", "`toggle_direction`"),
            ("auto_open = \"yes\"\n", "`auto_open`"),
            ("github_host = \"https://github.example.com\"\n", "`github_host`"),
            ("github_host = \"github.com\"\n", "`github_host`"),
            ("github_host = \"github.com-work\"\n", "`github_host`"),
            ("switcher_roots = \"~\"\n", "`switcher_roots`"),
            ("switcher_roots = [\"\"]\n", "`switcher_roots`"),
            ("switcher_roots = [\"~\", 1]\n", "`switcher_roots`"),
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for (text, key) in cases {
            std::fs::write(&path, text).unwrap();
            let error = super::plugin_config_in(dir.path()).unwrap_err().to_string();
            assert!(error.contains(key), "{text}: {error}");
            assert!(error.contains("expected"), "{text}: {error}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_config_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.toml")).unwrap();
        let error = super::plugin_config_in(dir.path()).unwrap_err().to_string();
        assert!(error.contains("read failed"));
        assert!(error.contains("config.toml"));
    }

    #[test]
    fn normalized_json_contains_every_key() {
        let value = PluginConfig::default().to_json();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 9);
        assert_eq!(object["toggle_placement"], "split");
        assert_eq!(object["toggle_direction"], "right");
        assert_eq!(object["auto_open"], true);
        assert!(object["github_host"].is_null());
        assert!(object["gitlab_host"].is_null());
        assert!(object["deep_pi_model"].is_null());
        assert_eq!(object["switcher_roots"], serde_json::json!([]));
    }

    #[test]
    fn deep_pi_model_parses_and_rejects_emptiness() {
        use super::plugin_config_in;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "deep_pi_model = \"openai-codex/gpt-5.6-sol\"\n").unwrap();
        let config = plugin_config_in(dir.path()).unwrap();
        assert_eq!(config.deep_pi_model(), Some("openai-codex/gpt-5.6-sol"));

        std::fs::write(&path, "deep_pi_model = \"  \"\n").unwrap();
        let error = plugin_config_in(dir.path()).unwrap_err();
        assert!(error.to_string().contains("deep_pi_model"), "{error}");

        std::fs::write(&path, "deep_pi_model = 5\n").unwrap();
        assert!(plugin_config_in(dir.path()).is_err(), "a non-string is rejected");
    }
}
