//! The collaboration session store: one JSON document per review target, owned exclusively.
//!
//! Everything a Deep Review session must not lose — drafts, ownership, tray, aliases,
//! follow state, location, edit history, worktree and Pi identity — persists here after
//! every meaningful mutation. The store lives under the reviewr state directory, outside
//! any reviewed worktree, and every write replaces the file atomically (temp + rename), so
//! a crash leaves either the previous complete state or the new one, never a torn file.
//! A claim records which process owns drafting for the target; another live reviewr reading
//! the store sees the claim and stays browse-only, so two instances can never publish
//! duplicate feedback.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// One target's persistent collaboration document.
#[derive(Clone, Debug)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    /// The store file for a collaboration target key, under `state`.
    pub fn for_target(state: &Path, target_key: &str) -> Self {
        let file = format!("{}.json", super::materialize::key_hash(target_key));
        Self { path: state.join("sessions").join(file) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The persisted document, when one exists and parses. A torn or corrupt file reads as
    /// absent — the previous complete state was already replaced atomically, so this only
    /// happens to hand-edited files.
    pub fn load(&self) -> Option<Value> {
        let bytes = std::fs::read(&self.path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Replace the document atomically.
    pub fn save(&self, doc: &Value) -> Result<(), String> {
        let Some(dir) = self.path.parent() else {
            return Err("store path has no parent".to_string());
        };
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|error| error.to_string())
    }

    /// Mutate the document as one read-modify-write, exclusive against every other process
    /// touching this store — the deep workspace persisting its state, the origin merging a
    /// handoff. Atomic saves alone cannot prevent two concurrent load–save windows from
    /// silently dropping one writer's changes. A missing document mutates from the empty
    /// shell, so seeding and updating are the same operation.
    pub fn update(&self, mutate: impl FnOnce(&mut Value)) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut doc = self.load().unwrap_or_else(|| json!({"v": 1}));
        mutate(&mut doc);
        self.save(&doc)
    }

    /// Hold the store's cross-process lock for the caller's scope. An OS file lock, not a
    /// lock file: the kernel releases it with the process, so a crashed holder can never
    /// wedge the target.
    fn lock(&self) -> Result<std::fs::File, String> {
        let Some(dir) = self.path.parent() else {
            return Err("store path has no parent".to_string());
        };
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        // Open read+write without truncating. Truncation of a file another handle holds
        // locked fails outright on Windows instead of waiting its turn — and append-only
        // is not an option either: LockFileEx demands GENERIC_READ or GENERIC_WRITE on
        // the handle, which an append handle lacks, so locking it is ERROR_ACCESS_DENIED.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.path.with_extension("json.lock"))
            .map_err(|error| error.to_string())?;
        file.lock().map_err(|error| error.to_string())?;
        Ok(file)
    }

    /// Remove the document (End Deep Review).
    pub fn delete(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    /// Claim exclusive draft ownership for `owner`. Fails while another live process holds
    /// the claim; a claim whose process died is stale and falls to the caller. Returns the
    /// current document (or an empty shell) with the claim applied and saved.
    pub fn claim(&self, owner: &str) -> Result<Value, String> {
        let _lock = self.lock()?;
        let mut doc = self.load().unwrap_or_else(|| json!({"v": 1}));
        if let Some(holder) = doc["owner"].as_object() {
            let id = holder.get("id").and_then(Value::as_str).unwrap_or_default();
            let pid = holder.get("pid").and_then(Value::as_u64).unwrap_or(0);
            if id != owner && pid_alive(pid) {
                return Err(format!("draft ownership is held by {id} (pid {pid})"));
            }
        }
        doc["owner"] = json!({"id": owner, "pid": std::process::id()});
        self.save(&doc)?;
        Ok(doc)
    }

    /// Whether another live process currently owns drafting for this target.
    pub fn owned_elsewhere(&self, me: &str) -> bool {
        let Some(doc) = self.load() else { return false };
        let Some(holder) = doc["owner"].as_object() else { return false };
        let id = holder.get("id").and_then(Value::as_str).unwrap_or_default();
        let pid = holder.get("pid").and_then(Value::as_u64).unwrap_or(0);
        id != me && pid_alive(pid)
    }

    /// Release the claim while keeping the state (Close, not End). A missing store stays
    /// missing — releasing is never a reason to create one.
    pub fn release(&self, owner: &str) {
        let Ok(_lock) = self.lock() else { return };
        if let Some(mut doc) = self.load()
            && doc["owner"]["id"].as_str() == Some(owner)
        {
            doc["owner"] = Value::Null;
            let _ = self.save(&doc);
        }
    }
}

/// Whether a pid names a live process — Unix asks `kill -0`, Windows asks `tasklist`.
/// A crashed session's claim must fall on every platform, or its target stays locked
/// forever; only a platform with neither probe errs toward not stealing ownership.
fn pid_alive(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }
    if cfg!(unix) {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .is_ok_and(|out| out.status.success())
    } else if cfg!(windows) {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()))
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_round_trip_and_replace_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::for_target(dir.path(), "github:x/o/r#5");
        assert!(store.load().is_none(), "no file, no state");

        let doc = json!({"v": 1, "target": "github:x/o/r#5", "drafts": [{"body": "hi"}]});
        store.save(&doc).unwrap();
        assert_eq!(store.load().unwrap()["drafts"][0]["body"], "hi");

        // The atomic replace leaves no temp debris and fully swaps content.
        let doc2 = json!({"v": 1, "drafts": []});
        store.save(&doc2).unwrap();
        assert_eq!(store.load().unwrap()["drafts"].as_array().unwrap().len(), 0);
        let entries: Vec<_> = std::fs::read_dir(store.path().parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries
                .iter()
                .all(|name| std::path::Path::new(name).extension() != Some("tmp".as_ref())),
            "{entries:?}"
        );
    }

    #[test]
    fn a_corrupt_file_reads_as_absent_rather_than_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::for_target(dir.path(), "k");
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"{ torn").unwrap();
        assert!(store.load().is_none());
    }

    #[test]
    fn claims_are_exclusive_against_live_owners_and_fall_from_dead_ones() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::for_target(dir.path(), "k");

        // Another *live* process (this test process) holds the claim: rejected.
        store
            .save(&json!({"v": 1, "owner": {"id": "other-session", "pid": std::process::id()}}))
            .unwrap();
        let error = store.claim("me").unwrap_err();
        assert!(error.contains("other-session"), "{error}");
        assert!(store.owned_elsewhere("me"));

        // A dead process's claim is stale: the claim falls to the next session.
        let mut dead = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "exit"]).spawn().unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let dead_pid = dead.id();
        dead.wait().unwrap();
        store.save(&json!({"v": 1, "owner": {"id": "gone", "pid": dead_pid}})).unwrap();
        let doc = store.claim("me").unwrap();
        assert_eq!(doc["owner"]["id"], "me");
        assert!(!store.owned_elsewhere("me"));

        // The same owner re-claims freely (idempotent restart).
        store.claim("me").unwrap();

        // Release keeps the state but frees the claim.
        store.release("me");
        assert!(!store.owned_elsewhere("anyone"));
        assert!(store.load().is_some(), "close preserves state");
    }

    #[test]
    fn releasing_a_missing_store_does_not_create_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::for_target(dir.path(), "k");
        store.release("me");
        assert!(store.load().is_none());
    }

    #[test]
    fn concurrent_updates_lose_no_writes() {
        // Two processes read-modify-write this store — the deep workspace persisting and
        // the origin merging a handoff. Without the lock, overlapping load–save windows
        // drop increments; with it, every one lands.
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::for_target(dir.path(), "k");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..25 {
                        store
                            .update(|doc| {
                                let n = doc["n"].as_u64().unwrap_or(0);
                                doc["n"] = json!(n + 1);
                            })
                            .unwrap();
                    }
                });
            }
        });
        assert_eq!(store.load().unwrap()["n"], 100);
    }
}
