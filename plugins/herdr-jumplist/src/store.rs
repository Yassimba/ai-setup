use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::history::History;

/// Run one locked read-modify-write on the history persisted under
/// `state_dir` (HERDR_PLUGIN_STATE_DIR). The exclusive lock spans the whole
/// operation, so concurrent event hooks and jump actions (herdr spawns one
/// process per invocation) serialize.
pub fn update<T>(state_dir: PathBuf, apply: impl FnOnce(&mut History) -> T) -> Result<T> {
    fs::create_dir_all(&state_dir)
        .with_context(|| format!("creating state dir {}", state_dir.display()))?;
    let lock_path = state_dir.join("history.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening lock file {}", lock_path.display()))?;
    lock.lock().context("locking history state")?;
    let state_path = state_dir.join("history.json");
    let mut history = load(&state_path);
    let outcome = apply(&mut history);
    save(&state_path, &history)?;
    // The lock releases when `lock` drops (also on early error returns).
    Ok(outcome)
}

fn load(state_path: &Path) -> History {
    // Missing or corrupt state degrades to an empty history rather than
    // wedging navigation forever.
    let mut history = fs::read(state_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<History>(&bytes).ok())
        .unwrap_or_default();
    history.sanitize();
    history
}

fn save(state_path: &Path, history: &History) -> Result<()> {
    let tmp_path = state_path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(history).context("serializing history")?;
    fs::write(&tmp_path, bytes).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, state_path).with_context(|| format!("replacing {}", state_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Direction;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("herdr-jumplist-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn state_round_trips_across_updates() {
        let dir = temp_dir("round-trip");
        update(dir.clone(), |history| history.record("a")).unwrap();
        update(dir.clone(), |history| history.record("b")).unwrap();
        let target = update(dir.clone(), |history| {
            history.jump(Direction::Back, |_| true)
        })
        .unwrap();
        assert_eq!(target, Some("a".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_state_degrades_to_empty_history() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("history.json"), b"{not json").unwrap();
        let target = update(dir.clone(), |history| {
            history.jump(Direction::Back, |_| true)
        })
        .unwrap();
        assert_eq!(target, None);
        let _ = fs::remove_dir_all(&dir);
    }
}
