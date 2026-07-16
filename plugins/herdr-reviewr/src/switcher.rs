//! The built-in project switcher: discover candidate repos, rank and filter them.
//!
//! See `specs/tui.md#project-switcher`. Everything here is local — a directory scan plus
//! an optional zoxide ranking — and powers the overlay in [`crate::app`]. The pick only
//! re-points this sidebar; no other pane is touched.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::Command;

/// One candidate project: a top-level directory under a configured root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
}

/// Expand a leading `~` against the home directory (`HOME`, else Windows' `USERPROFILE`).
/// `~` alone, `~/`, and Windows-style `~\` all expand; anything else (`~user`) passes through.
pub fn expand_tilde(raw: &str) -> PathBuf {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"));
    if let Some(home) = home {
        if raw == "~" {
            return PathBuf::from(home);
        }
        if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix(r"~\")) {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw)
}

/// The non-hidden top-level directories under each root, zoxide-frecency first, the
/// remainder alphabetical. A missing or failing `zoxide` degrades to alphabetical only.
pub fn discover(roots: &[PathBuf]) -> Vec<Project> {
    ranked(scan(roots), &zoxide_rank())
}

/// The raw directory scan, unranked.
fn scan(roots: &[PathBuf]) -> Vec<Project> {
    let mut projects = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            projects.push(Project { name: name.to_owned(), path });
        }
    }
    projects
}

/// Order `projects` by `rank` (lower first), ties alphabetical, duplicates dropped.
fn ranked(mut projects: Vec<Project>, rank: &HashMap<PathBuf, usize>) -> Vec<Project> {
    projects.sort_by(|a, b| {
        let ra = rank.get(&a.path).copied().unwrap_or(usize::MAX);
        let rb = rank.get(&b.path).copied().unwrap_or(usize::MAX);
        ra.cmp(&rb).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    projects.dedup_by(|a, b| a.path == b.path);
    projects
}

/// Paths in zoxide's frecency order, best first. `zoxide query -l` prints one path per
/// line; the line index is the rank.
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
        .map(|(index, line)| (PathBuf::from(line.trim_end_matches('/')), index))
        .collect()
}

/// The indices of `projects` whose name matches `query`, best match first: substring
/// matches (earliest occurrence first) ahead of looser in-order subsequence matches,
/// ties keeping the incoming (frecency) order. Case-insensitive; an empty query keeps
/// every project in order.
pub fn filter(projects: &[Project], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return (0..projects.len()).collect();
    }
    let mut scored: Vec<(usize, usize, usize)> = projects
        .iter()
        .enumerate()
        .filter_map(|(index, project)| {
            let (tier, at) = fuzzy_score(&project.name, &query)?;
            Some((tier, at, index))
        })
        .collect();
    scored.sort_unstable();
    scored.into_iter().map(|(_, _, index)| index).collect()
}

/// Rank one candidate against a query. Contiguous substring matches win over looser
/// left-to-right subsequences; within a tier, an earlier start wins. Shared by the project and
/// PR/MR pickers so "fuzzy" has one meaning throughout the TUI.
pub fn fuzzy_score(candidate: &str, query: &str) -> Option<(usize, usize)> {
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some((0, 0));
    }
    match candidate.find(&query) {
        Some(at) => Some((0, at)),
        None => Some((1, subsequence_at(&candidate, &query)?)),
    }
}

/// Where a left-to-right subsequence match of `query` starts in `name`, or `None`.
fn subsequence_at(name: &str, query: &str) -> Option<usize> {
    let mut wanted = query.chars();
    let mut current = wanted.next()?;
    let mut start = None;
    for (at, c) in name.char_indices() {
        if c != current {
            continue;
        }
        start.get_or_insert(at);
        match wanted.next() {
            Some(next) => current = next,
            None => return start,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Project, expand_tilde, filter, fuzzy_score, ranked, scan};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn projects(names: &[&str]) -> Vec<Project> {
        names
            .iter()
            .map(|n| Project { name: (*n).to_string(), path: PathBuf::from(format!("/p/{n}")) })
            .collect()
    }

    #[test]
    fn tilde_expands_alone_before_slash_and_before_backslash() {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .expect("test environments define a home");
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/projects"), home.join("projects"));
        assert_eq!(expand_tilde(r"~\projects"), home.join("projects"));
        // Not a home reference: `~user` and non-tilde paths pass through untouched.
        assert_eq!(expand_tilde("~other/projects"), PathBuf::from("~other/projects"));
        assert_eq!(expand_tilde("/absolute/path"), PathBuf::from("/absolute/path"));
    }

    #[test]
    fn scan_lists_non_hidden_top_level_directories_only() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("alpha")).unwrap();
        std::fs::create_dir(root.path().join(".hidden")).unwrap();
        std::fs::write(root.path().join("file.txt"), "x").unwrap();

        let found = scan(&[root.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "alpha");
        assert_eq!(found[0].path, root.path().join("alpha"));
    }

    #[test]
    fn ranked_puts_frecency_first_then_alphabetical_and_dedups() {
        let list = projects(&["zeta", "alpha", "midway", "alpha"]);
        let rank: HashMap<PathBuf, usize> = [(PathBuf::from("/p/zeta"), 0)].into();
        let names: Vec<String> = ranked(list, &rank).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["zeta", "alpha", "midway"]);
    }

    #[test]
    fn missing_zoxide_degrades_to_case_insensitive_alphabetical() {
        // An absent or failing `zoxide` makes `zoxide_rank` an empty map; every project then
        // ties on rank and the ordering must be case-insensitive alphabetical, not an error.
        let list = projects(&["zeta", "Alpha", "midway"]);
        let names: Vec<String> =
            ranked(list, &HashMap::new()).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Alpha", "midway", "zeta"]);
    }

    #[test]
    fn empty_query_keeps_every_project_in_order() {
        assert_eq!(filter(&projects(&["b", "a"]), ""), [0, 1]);
    }

    #[test]
    fn substring_matches_outrank_subsequence_matches() {
        // "gia" is a substring of "magia" but only a subsequence of "gaia-builder".
        let list = projects(&["gaia-builder", "magia"]);
        assert_eq!(filter(&list, "gia"), [1, 0]);
    }

    #[test]
    fn earlier_substring_positions_win_and_ties_keep_frecency_order() {
        let list = projects(&["herdr-reviewr", "reviewr", "reviewr-fork"]);
        // "reviewr" starts at 0 in the last two (kept in incoming order), at 6 in the first.
        assert_eq!(filter(&list, "reviewr"), [1, 2, 0]);
    }

    #[test]
    fn fuzzy_score_is_shared_without_project_specific_state() {
        assert!(fuzzy_score("!42 Fix Login alice feature/auth", "fixlog").is_some());
        assert_eq!(fuzzy_score("feature/auth", "AUTH"), Some((0, 8)));
        assert_eq!(fuzzy_score("feature/auth", "xyz"), None);
    }

    #[test]
    fn filter_is_case_insensitive_and_drops_non_matches() {
        let list = projects(&["Skills", "wiki"]);
        assert_eq!(filter(&list, "SKI"), [0]);
        assert_eq!(filter(&list, "xyz"), Vec::<usize>::new());
    }
}
