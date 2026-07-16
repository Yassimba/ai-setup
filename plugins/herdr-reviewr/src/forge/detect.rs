//! Provider auto-detection: which forge owns an origin host.
//!
//! The trust rule (`specs/forge-host.md`): a host is a GitHub host if the user authenticated
//! `gh` against it, a GitLab host if they authenticated `glab` — both CLIs keep a per-host
//! registry in a local config file. Detection is a file read: no subprocess, no network. The
//! well-known hosts need no lookup, and the explicit `github_host`/`gitlab_host` config keys
//! stay as overrides for edge cases (and the tiebreaker when both CLIs claim one host).

use std::path::{Path, PathBuf};

use super::Provider;

/// The provider verdict for one origin host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Provider(Provider),
    /// No well-known host, no override, and neither CLI claims it.
    Unknown,
    /// Both CLIs claim the host and no override breaks the tie.
    Ambiguous,
}

/// One immutable snapshot of every host the fetch may trust, loaded per fetch so a fresh
/// `gh`/`glab auth login` is picked up by the next refresh without restarting the sidebar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostClassifier {
    github_hosts: Vec<String>,
    gitlab_hosts: Vec<String>,
    github_override: Option<String>,
    gitlab_override: Option<String>,
}

impl HostClassifier {
    /// Load the CLI host registries from their platform locations plus the config overrides.
    pub fn load(config: &crate::config::PluginConfig) -> Self {
        Self::from_files(
            gh_hosts_path().as_deref(),
            glab_config_path().as_deref(),
            config.github_host(),
            config.gitlab_host(),
        )
    }

    /// The explicit constructor: file paths (either may be absent) and overrides. Public so
    /// integration tests can build a hermetic classifier without touching the real config.
    pub fn from_files(
        gh_hosts: Option<&Path>,
        glab_config: Option<&Path>,
        github_override: Option<&str>,
        gitlab_override: Option<&str>,
    ) -> Self {
        Self {
            github_hosts: gh_hosts.map(top_level_yaml_keys).unwrap_or_default(),
            gitlab_hosts: glab_config.map(|p| yaml_keys_under(p, "hosts")).unwrap_or_default(),
            github_override: github_override.map(str::to_ascii_lowercase),
            gitlab_override: gitlab_override.map(str::to_ascii_lowercase),
        }
    }

    /// Every canonical host this classifier could resolve — the alias-expansion set for
    /// `git.rs`'s ssh-alias handling (`github.com-work` → `github.com`).
    pub(crate) fn known_hosts(&self) -> impl Iterator<Item = &str> {
        ["github.com", "gitlab.com"]
            .into_iter()
            .chain(self.github_override.as_deref())
            .chain(self.gitlab_override.as_deref())
            .chain(self.github_hosts.iter().map(String::as_str))
            .chain(self.gitlab_hosts.iter().map(String::as_str))
    }

    /// The provider verdict for one canonical (already alias-resolved, lowercase) host.
    /// Precedence: well-known → explicit override → CLI-claimed; both CLIs without an
    /// override is ambiguous rather than a silent guess.
    pub fn classify(&self, host: &str) -> Classification {
        let host = host.to_ascii_lowercase();
        if host == "github.com" {
            return Classification::Provider(Provider::Github);
        }
        if host == "gitlab.com" {
            return Classification::Provider(Provider::Gitlab);
        }
        let github_override = self.github_override.as_deref() == Some(host.as_str());
        let gitlab_override = self.gitlab_override.as_deref() == Some(host.as_str());
        match (github_override, gitlab_override) {
            (true, true) => return Classification::Ambiguous,
            (true, false) => return Classification::Provider(Provider::Github),
            (false, true) => return Classification::Provider(Provider::Gitlab),
            (false, false) => {}
        }
        let gh = self.github_hosts.iter().any(|h| h == &host);
        let glab = self.gitlab_hosts.iter().any(|h| h == &host);
        match (gh, glab) {
            (true, true) => Classification::Ambiguous,
            (true, false) => Classification::Provider(Provider::Github),
            (false, true) => Classification::Provider(Provider::Gitlab),
            (false, false) => Classification::Unknown,
        }
    }
}

/// `gh`'s per-host auth registry: `hosts.yml` under `GH_CONFIG_DIR`, else the platform
/// config home (`~/.config/gh` on unix, `%AppData%\GitHub CLI` on Windows).
fn gh_hosts_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GH_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("hosts.yml"));
    }
    #[cfg(windows)]
    {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("GitHub CLI").join("hosts.yml"))
    }
    #[cfg(not(windows))]
    {
        Some(unix_config_home()?.join("gh").join("hosts.yml"))
    }
}

/// `glab`'s config with its `hosts:` map: `config.yml` under `GLAB_CONFIG_DIR`, else the
/// platform config home. On macOS glab uses `~/Library/Application Support/glab-cli` (its Go
/// `os.UserConfigDir`), on linux `~/.config/glab-cli`, on Windows `%AppData%\glab-cli`.
fn glab_config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GLAB_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("config.yml"));
    }
    #[cfg(windows)]
    {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("glab-cli").join("config.yml"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        let native = PathBuf::from(&home)
            .join("Library")
            .join("Application Support")
            .join("glab-cli")
            .join("config.yml");
        if native.exists() {
            return Some(native);
        }
        // Older glab versions (and XDG-configured setups) used ~/.config/glab-cli.
        Some(PathBuf::from(home).join(".config").join("glab-cli").join("config.yml"))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Some(unix_config_home()?.join("glab-cli").join("config.yml"))
    }
}

#[cfg(not(windows))]
fn unix_config_home() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config"))
}

/// The top-level keys of a small YAML mapping file — `gh`'s `hosts.yml` shape, where every
/// root key is an authenticated hostname. A tolerant line scan, not a YAML parser: quoted
/// keys are unquoted, comments and blank lines skipped, non-mapping lines ignored. An
/// unreadable file reads as no hosts (detection then falls back to the config overrides).
fn top_level_yaml_keys(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    mapping_keys(&content, None)
}

/// The keys one level under a root `section:` — `glab`'s `hosts:` map in `config.yml`.
fn yaml_keys_under(path: &Path, section: &str) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    mapping_keys(&content, Some(section))
}

/// Scan mapping keys: at the root (no `section`), or the first indent level inside `section`.
/// The child indent is whatever the section's first child uses, so 2- and 4-space files both
/// parse. Keys that don't look like hostnames (no dot, or containing spaces) are kept anyway —
/// classification compares exact strings, so a stray `oauth_token:` key can never match a host.
fn mapping_keys(content: &str, section: Option<&str>) -> Vec<String> {
    let mut keys = Vec::new();
    let mut in_section = section.is_none();
    let mut child_indent: Option<usize> = None;
    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        let body = trimmed.trim_start();
        if let Some(section) = section {
            if indent == 0 {
                in_section = body == format!("{section}:");
                child_indent = None;
                continue;
            }
            if !in_section {
                continue;
            }
            // The first indented line under the section sets the key indent; deeper lines
            // are the keys' own nested values.
            let expected = *child_indent.get_or_insert(indent);
            if indent != expected {
                continue;
            }
        } else if indent != 0 {
            continue;
        }
        let Some(key) = body.strip_suffix(':').or_else(|| body.split_once(": ").map(|(k, _)| k))
        else {
            continue;
        };
        let key = key.trim_matches(|c| c == '"' || c == '\'');
        if !key.is_empty() {
            keys.push(key.to_ascii_lowercase());
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    const GH_HOSTS: &str = "\
github.com:
    user: yassimba
    oauth_token: gho_x
    git_protocol: ssh
\"github.example.com\":
    user: yassin
";

    const GLAB_CONFIG: &str = "\
git_protocol: ssh
check_update: true
hosts:
    gitlab.com:
        token: glpat-x
        api_protocol: https
    gitlab.selfhosted.example.com:
        token: glpat-y
";

    #[test]
    fn well_known_hosts_classify_without_any_files() {
        let c = HostClassifier::from_files(None, None, None, None);
        assert_eq!(c.classify("github.com"), Classification::Provider(Provider::Github));
        assert_eq!(c.classify("gitlab.com"), Classification::Provider(Provider::Gitlab));
        assert_eq!(c.classify("GitHub.COM"), Classification::Provider(Provider::Github));
        assert_eq!(c.classify("git.example.com"), Classification::Unknown);
    }

    #[test]
    fn cli_config_files_claim_their_authenticated_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let gh = write(dir.path(), "hosts.yml", GH_HOSTS);
        let glab = write(dir.path(), "config.yml", GLAB_CONFIG);
        let c = HostClassifier::from_files(Some(&gh), Some(&glab), None, None);
        assert_eq!(
            c.classify("github.example.com"),
            Classification::Provider(Provider::Github),
            "gh's quoted host key is unquoted and claimed"
        );
        assert_eq!(
            c.classify("gitlab.selfhosted.example.com"),
            Classification::Provider(Provider::Gitlab),
            "glab's hosts: map claims a self-hosted GitLab"
        );
        // glab's top-level scalar keys (git_protocol, check_update) never leak into hosts.
        assert_eq!(c.classify("git_protocol"), Classification::Unknown);
        // A host in neither registry stays unknown.
        assert_eq!(c.classify("bitbucket.org"), Classification::Unknown);
    }

    #[test]
    fn overrides_win_and_break_double_claims() {
        let dir = tempfile::tempdir().unwrap();
        // Both CLIs claim the same host: ambiguous without an override…
        let gh = write(dir.path(), "hosts.yml", "git.example.com:\n    user: y\n");
        let glab =
            write(dir.path(), "config.yml", "hosts:\n    git.example.com:\n        token: t\n");
        let both = HostClassifier::from_files(Some(&gh), Some(&glab), None, None);
        assert_eq!(both.classify("git.example.com"), Classification::Ambiguous);
        // …and resolved by one.
        let tie = HostClassifier::from_files(Some(&gh), Some(&glab), None, Some("git.example.com"));
        assert_eq!(tie.classify("git.example.com"), Classification::Provider(Provider::Gitlab));
        // An override also works with no CLI file at all (the pre-detection contract).
        let cfg_only = HostClassifier::from_files(None, None, Some("github.example.com"), None);
        assert_eq!(
            cfg_only.classify("github.example.com"),
            Classification::Provider(Provider::Github)
        );
    }

    #[test]
    fn missing_or_unreadable_files_read_as_no_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("nope.yml");
        let c = HostClassifier::from_files(Some(&gone), Some(&gone), None, None);
        assert_eq!(c.classify("github.example.com"), Classification::Unknown);
    }

    #[test]
    fn glab_hosts_parse_with_two_space_indent_too() {
        let dir = tempfile::tempdir().unwrap();
        let glab = write(
            dir.path(),
            "config.yml",
            "hosts:\n  gitlab.example.com:\n    token: t\n  other.example.com:\n    token: u\n",
        );
        let c = HostClassifier::from_files(None, Some(&glab), None, None);
        assert_eq!(c.classify("gitlab.example.com"), Classification::Provider(Provider::Gitlab));
        assert_eq!(c.classify("other.example.com"), Classification::Provider(Provider::Gitlab));
    }

    #[test]
    fn known_hosts_expose_the_alias_expansion_set() {
        let dir = tempfile::tempdir().unwrap();
        let glab =
            write(dir.path(), "config.yml", "hosts:\n    selfhosted.example.com:\n        token: t\n");
        let c = HostClassifier::from_files(None, Some(&glab), Some("ghe.example.com"), None);
        let known: Vec<&str> = c.known_hosts().collect();
        assert!(known.contains(&"github.com"));
        assert!(known.contains(&"gitlab.com"));
        assert!(known.contains(&"ghe.example.com"));
        assert!(known.contains(&"selfhosted.example.com"));
    }
}
