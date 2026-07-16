use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::projects;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Directories whose immediate children are projects. Empty means auto-detect.
    pub roots: Vec<String>,
    /// Windows target shell: "powershell" (default) or "cmd".
    pub windows_shell: WindowsShell,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WindowsShell {
    #[default]
    Powershell,
    Cmd,
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
                eprintln!("ignoring invalid {}: {err}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn resolved_roots(&self) -> Vec<PathBuf> {
        let configured: Vec<_> = self
            .roots
            .iter()
            .map(|root| projects::expand_tilde(root))
            .filter(|root| root.is_dir())
            .collect();
        if configured.is_empty() && self.roots.is_empty() {
            projects::detect_roots()
        } else {
            configured
        }
    }

    pub fn save_roots(&mut self, roots: &[PathBuf]) -> Result<()> {
        self.roots = roots
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect();
        let path = config_path().context("HERDR_PLUGIN_CONFIG_DIR is not set")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR")
        .map(PathBuf::from)
        .map(|dir| dir.join("config.toml"))
}
