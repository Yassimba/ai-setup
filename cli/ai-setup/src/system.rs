use crate::CommandSpec;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait System {
    fn command_exists(&self, name: &str) -> bool;
    fn refresh_path(&self);
    fn run(&self, command: &CommandSpec) -> Result<CommandResult>;
    /// The home directory skill trees are detected under. Injectable so
    /// tests can point the installer at a temp home.
    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }
}

/// Holds the PATH this process resolves tools against. It starts as the
/// inherited PATH and grows as installers register new tool directories.
/// Kept here rather than in the process environment: the wizard refreshes
/// it from the install worker thread while the UI thread keeps running, and
/// `env::set_var` racing environment reads is undefined behavior on Unix.
pub struct RealSystem {
    path: Mutex<OsString>,
}

impl Default for RealSystem {
    fn default() -> Self {
        Self {
            path: Mutex::new(env::var_os("PATH").unwrap_or_default()),
        }
    }
}

impl RealSystem {
    fn path_value(&self) -> OsString {
        self.path.lock().expect("PATH lock poisoned").clone()
    }
}

/// Locate `name` on `path` the way the shell would, honoring PATHEXT on
/// Windows so `.cmd`/`.bat` shims (npm, pi) are found.
fn resolve_program(path: &OsStr, name: &str) -> Option<PathBuf> {
    let extensions = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        vec![String::new()]
    };
    env::split_paths(path).find_map(|directory| {
        extensions.iter().find_map(|extension| {
            let candidate = Path::new(&directory).join(format!("{name}{extension}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

/// Build a std Command for the spec. On Windows the program must be resolved
/// to its full path first: CreateProcess only appends `.exe`, so a bare
/// `npm`/`pi` never finds the `.cmd` shims those tools install as.
/// The child also gets `path` as its PATH; on Unix std looks the bare
/// program name up against that override.
fn command_for(path: &OsStr, spec: &CommandSpec) -> Command {
    let program = if cfg!(windows) {
        resolve_program(path, &spec.program).unwrap_or_else(|| PathBuf::from(&spec.program))
    } else {
        PathBuf::from(&spec.program)
    };
    let mut command = Command::new(program);
    command.args(&spec.args);
    command.env("PATH", path);
    command
}

/// PATH entries persisted to the Windows registry. Installers (Herdr, npm)
/// append their bin directory there, but this process only sees the PATH it
/// was started with, so re-read the registry after installers run.
fn registry_path_entries() -> Vec<PathBuf> {
    let Ok(output) = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "[Environment]::GetEnvironmentVariable('Path', 'User'); \
             [Environment]::GetEnvironmentVariable('Path', 'Machine')",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

impl System for RealSystem {
    fn command_exists(&self, name: &str) -> bool {
        resolve_program(&self.path_value(), name).is_some()
    }

    fn refresh_path(&self) {
        let mut paths = env::split_paths(&self.path_value()).collect::<Vec<_>>();
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".local").join("bin"));
            paths.push(home.join(".cargo").join("bin"));
            if cfg!(windows) {
                paths.push(home.join("AppData").join("Roaming").join("npm"));
            }
        }
        if cfg!(windows) {
            paths.extend(registry_path_entries());
        }
        // Probe npm against the merged candidates, so an npm that only just
        // appeared in the registry PATH still reports its global prefix.
        let merged = env::join_paths(paths.iter().cloned()).unwrap_or_else(|_| self.path_value());
        if let Ok(output) =
            command_for(&merged, &CommandSpec::new("npm", ["prefix", "--global"])).output()
        {
            if output.status.success() {
                let prefix = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if !prefix.is_empty() {
                    let prefix = PathBuf::from(prefix);
                    paths.push(if cfg!(windows) {
                        prefix
                    } else {
                        prefix.join("bin")
                    });
                }
            }
        }
        paths.retain(|path| path.is_dir());
        let mut seen = HashSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        if let Ok(path) = env::join_paths(paths) {
            *self.path.lock().expect("PATH lock poisoned") = path;
        }
    }

    fn run(&self, command: &CommandSpec) -> Result<CommandResult> {
        let output = command_for(&self.path_value(), command)
            .output()
            .with_context(|| format!("could not start {}", command.program))?;
        Ok(CommandResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
