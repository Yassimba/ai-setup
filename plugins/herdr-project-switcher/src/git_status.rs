use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};

const WORKERS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoState {
    Clean,
    Changed,
    Conflicted,
    NotRepository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectGitStatus {
    pub state: RepoState,
    pub added: u64,
    pub removed: u64,
    pub untracked: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Staged,
    Modified,
    Untracked,
    Conflicted,
}

#[derive(Debug, Default)]
pub struct FileStateIndex {
    paths: HashMap<PathBuf, FileState>,
}

impl FileStateIndex {
    /// State for a visible path. Directories inherit the highest-priority state
    /// from changed descendants so collapsed folders still advertise changes.
    pub fn state_for(&self, relative: &Path, directory: bool) -> Option<FileState> {
        self.paths.get(relative).copied().or_else(|| {
            directory
                .then(|| {
                    self.paths
                        .iter()
                        .filter(|(path, _)| path.starts_with(relative))
                        .map(|(_, state)| *state)
                        .max_by_key(|state| file_state_priority(*state))
                })
                .flatten()
        })
    }
}

/// Git state for files in the selected project's preview. `--untracked-files=all`
/// expands untracked directories so their visible children can be colored too.
pub fn file_states(path: &Path) -> FileStateIndex {
    let Ok(output) = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    else {
        return FileStateIndex::default();
    };
    if !output.status.success() {
        return FileStateIndex::default();
    }
    file_states_from_porcelain(&output.stdout)
}

/// Inspect project repositories in a bounded background pool. Results arrive
/// independently so the picker remains interactive while rows fill in.
pub fn scan(
    paths: impl IntoIterator<Item = PathBuf>,
) -> mpsc::Receiver<(PathBuf, ProjectGitStatus)> {
    let queue: VecDeque<_> = paths.into_iter().collect();
    let worker_count = WORKERS.min(queue.len());
    let queue = Arc::new(Mutex::new(queue));
    let (sender, receiver) = mpsc::channel();

    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        std::thread::spawn(move || {
            while let Some(path) = queue.lock().ok().and_then(|mut queue| queue.pop_front()) {
                if sender.send((path.clone(), inspect(&path))).is_err() {
                    break;
                }
            }
        });
    }
    drop(sender);
    receiver
}

fn inspect(path: &Path) -> ProjectGitStatus {
    let Ok(status) = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
        .output()
    else {
        return not_repository();
    };
    if !status.status.success() {
        return not_repository();
    }

    let (state, untracked) = parse_porcelain(&status.stdout);
    if state == RepoState::Clean {
        return ProjectGitStatus {
            state,
            added: 0,
            removed: 0,
            untracked,
        };
    }

    let (added, removed) = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["diff", "--numstat", "HEAD", "--"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_numstat(&output.stdout))
        .unwrap_or_default();
    ProjectGitStatus {
        state,
        added,
        removed,
        untracked,
    }
}

fn not_repository() -> ProjectGitStatus {
    ProjectGitStatus {
        state: RepoState::NotRepository,
        added: 0,
        removed: 0,
        untracked: 0,
    }
}

fn parse_porcelain(output: &[u8]) -> (RepoState, usize) {
    let records = porcelain_records(output);
    let changed = !records.is_empty();
    let untracked = records
        .iter()
        .filter(|(x, y, _)| *x == b'?' && *y == b'?')
        .count();
    let conflicted = records.iter().any(|(x, y, _)| is_conflicted(*x, *y));
    let state = if conflicted {
        RepoState::Conflicted
    } else if changed {
        RepoState::Changed
    } else {
        RepoState::Clean
    };
    (state, untracked)
}

fn file_states_from_porcelain(output: &[u8]) -> FileStateIndex {
    let paths = porcelain_records(output)
        .into_iter()
        .map(|(x, y, path)| {
            let state = if is_conflicted(x, y) {
                FileState::Conflicted
            } else if x == b'?' && y == b'?' {
                FileState::Untracked
            } else if y != b' ' {
                FileState::Modified
            } else {
                FileState::Staged
            };
            (path, state)
        })
        .collect();
    FileStateIndex { paths }
}

fn porcelain_records(output: &[u8]) -> Vec<(u8, u8, PathBuf)> {
    let mut raw = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut records = Vec::new();
    while let Some(record) = raw.next() {
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        let x = record[0];
        let y = record[1];
        records.push((
            x,
            y,
            PathBuf::from(String::from_utf8_lossy(&record[3..]).into_owned()),
        ));
        // Porcelain v1 -z emits a second pathname record for renames/copies.
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            let _ = raw.next();
        }
    }
    records
}

fn is_conflicted(x: u8, y: u8) -> bool {
    matches!(
        (x, y),
        (b'D', b'D')
            | (b'A', b'U')
            | (b'U', b'D')
            | (b'U', b'A')
            | (b'D', b'U')
            | (b'A', b'A')
            | (b'U', b'U')
    )
}

fn file_state_priority(state: FileState) -> u8 {
    match state {
        FileState::Staged => 1,
        FileState::Untracked => 2,
        FileState::Modified => 3,
        FileState::Conflicted => 4,
    }
}

fn parse_numstat(output: &[u8]) -> (u64, u64) {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let added = fields.next()?.parse::<u64>().ok()?;
            let removed = fields.next()?.parse::<u64>().ok()?;
            Some((added, removed))
        })
        .fold(
            (0_u64, 0_u64),
            |(total_added, total_removed), (added, removed)| {
                (
                    total_added.saturating_add(added),
                    total_removed.saturating_add(removed),
                )
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_changed_and_conflicted_states() {
        assert_eq!(parse_porcelain(b""), (RepoState::Clean, 0));
        assert_eq!(
            parse_porcelain(b" M src/main.rs\0?? notes.txt\0"),
            (RepoState::Changed, 1)
        );
        assert_eq!(
            parse_porcelain(b"UU src/main.rs\0"),
            (RepoState::Conflicted, 0)
        );
    }

    #[test]
    fn skips_rename_source_records() {
        assert_eq!(
            parse_porcelain(b"R  new-name\0??-old-name\0?? real-untracked\0"),
            (RepoState::Changed, 1)
        );
    }

    #[test]
    fn totals_text_changes_and_ignores_binary_rows() {
        assert_eq!(
            parse_numstat(b"12\t3\tsrc/main.rs\n-\t-\timage.png\n5\t0\tREADME.md\n"),
            (17, 3)
        );
    }

    #[test]
    fn indexes_file_states_and_colors_parent_directories() {
        let index = file_states_from_porcelain(
            b"M  staged.rs\0 M src/modified.rs\0?? docs/new.md\0UU src/conflict.rs\0",
        );
        assert_eq!(
            index.state_for(Path::new("staged.rs"), false),
            Some(FileState::Staged)
        );
        assert_eq!(
            index.state_for(Path::new("docs/new.md"), false),
            Some(FileState::Untracked)
        );
        assert_eq!(
            index.state_for(Path::new("src"), true),
            Some(FileState::Conflicted)
        );
        assert_eq!(index.state_for(Path::new("unchanged.rs"), false), None);
    }
}
