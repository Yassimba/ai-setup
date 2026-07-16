use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
}

pub fn expand_tilde(raw: &str) -> PathBuf {
    if raw == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

/// Existing conventional project roots. We deliberately do not crawl the home
/// directory: when none exist, the UI asks the user to choose one.
pub fn detect_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    [
        "Projects",
        "projects",
        "Developer",
        "developer",
        "src",
        "code",
        "dev",
        "Documents/projects",
        "Documents/Projects",
    ]
    .into_iter()
    .map(|candidate| home.join(candidate))
    .filter(|candidate| candidate.is_dir())
    .collect()
}

/// Top-level directories under each root, zoxide-frecency first, then
/// alphabetical. A missing zoxide simply means alphabetical ordering.
pub fn discover(roots: &[PathBuf]) -> Vec<Project> {
    let mut projects = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !seen.insert(path.clone()) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            projects.push(Project {
                name: name.to_owned(),
                path,
            });
        }
    }
    let rank = zoxide_rank();
    projects.sort_by(|a, b| {
        rank.get(&normalized(&a.path))
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(
                &rank
                    .get(&normalized(&b.path))
                    .copied()
                    .unwrap_or(usize::MAX),
            )
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    projects
}

fn normalized(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn zoxide_rank() -> HashMap<PathBuf, usize> {
    let Ok(output) = Command::new("zoxide").args(["query", "-l"]).output() else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .enumerate()
        .map(|(index, line)| {
            (
                normalized(Path::new(line.trim_end_matches(['/', '\\']))),
                index,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion_leaves_plain_paths_unchanged() {
        assert_eq!(
            expand_tilde("relative/project"),
            PathBuf::from("relative/project")
        );
    }
}
