//! Remote review materialization: the exact provider head, checked out on a writable local
//! review branch in an isolated worktree.
//!
//! Both forges publish every review head — fork or same-repo — as a fetchable ref
//! (`refs/pull/N/head`, `refs/merge-requests/N/head`), so materialization never needs the
//! contributor's remote. Worktrees live under the reviewr state directory, outside any
//! reviewed checkout, so a branch cleanup or rebase in the source repo cannot take the Deep
//! Review workspace with it. Updates are explicit only: a clean fast-forward when possible,
//! else a rebase of the private review branch that stops on conflict — nothing here ever
//! resets an edit or resolves a conflict on its own.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::forge::Provider;

/// Run one git command, capturing stdout; a failure carries git's stderr.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|error| format!("git {}: {error}", args.first().unwrap_or(&"")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Where collaboration state (session files, deep worktrees) lives — always outside the
/// reviewed worktree. Overridable for tests and pinned setups.
pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("REVIEWR_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("HERDR_PLUGIN_STATE_DIR") {
        return PathBuf::from(dir).join("collab");
    }
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("reviewr");
    }
    if cfg!(windows)
        && let Ok(dir) = std::env::var("LOCALAPPDATA")
    {
        return PathBuf::from(dir).join("reviewr").join("state");
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
    PathBuf::from(home.unwrap_or_else(|_| ".".into())).join(".local/state/reviewr")
}

/// FNV-1a of a collaboration key, for stable directory names.
pub fn key_hash(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The fetchable ref every forge publishes for a review's exact head, forks included.
pub fn review_ref(provider: Provider, number: u64) -> String {
    match provider {
        Provider::Github => format!("refs/pull/{number}/head"),
        Provider::Gitlab => format!("refs/merge-requests/{number}/head"),
    }
}

/// One materialized Deep Review checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Materialized {
    pub worktree: PathBuf,
    pub branch: String,
    pub head: String,
    /// False when an existing valid worktree was reused.
    pub created: bool,
}

/// Fetch the review head and return its exact sha, without touching any checkout.
pub fn fetch_review_head(repo: &Path, provider: Provider, number: u64) -> Result<String, String> {
    git(repo, &["fetch", "--quiet", "origin", &review_ref(provider, number)])?;
    git(repo, &["rev-parse", "FETCH_HEAD"])
}

/// Materialize (or reuse) the Deep Review worktree for one remote target. Reuse validates
/// the existing checkout rather than trusting the directory; creation rolls itself back on
/// partial failure so a retry starts clean. The local review branch is private — nothing
/// here pushes it anywhere.
pub fn materialize(
    repo: &Path,
    state: &Path,
    provider: Provider,
    number: u64,
    target_key: &str,
) -> Result<Materialized, String> {
    let dir = state.join("worktrees").join(key_hash(target_key));
    if dir.join(".git").exists() {
        if let (Ok(head), Ok(branch)) =
            (git(&dir, &["rev-parse", "HEAD"]), git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]))
        {
            return Ok(Materialized { worktree: dir, branch, head, created: false });
        }
        // A corrupt leftover: clear it and rebuild rather than binding Pi to wreckage.
        let _ = git(repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    let head = fetch_review_head(repo, provider, number)?;
    std::fs::create_dir_all(state.join("worktrees")).map_err(|error| error.to_string())?;
    let provider_slug = match provider {
        Provider::Github => "pr",
        Provider::Gitlab => "mr",
    };
    // A private, writable review branch; on a name collision (an old session's branch that
    // lost its worktree) pick the next free suffix rather than resetting anything.
    let base = format!("reviewr/{provider_slug}-{number}");
    let branch = (0..10)
        .map(|i| if i == 0 { base.clone() } else { format!("{base}-{i}") })
        .find(|name| git(repo, &["rev-parse", "--verify", "--quiet", name]).is_err())
        .ok_or_else(|| format!("no free branch name near {base}"))?;
    let dir_arg = dir.to_string_lossy().into_owned();
    if let Err(error) = git(repo, &["worktree", "add", "-b", &branch, &dir_arg, &head]) {
        // Roll back whatever half-landed; ownership of the attempt stays with the caller.
        let _ = git(repo, &["worktree", "remove", "--force", &dir_arg]);
        let _ = git(repo, &["branch", "-D", &branch]);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error);
    }
    Ok(Materialized { worktree: dir, branch, head, created: true })
}

/// How an explicit update landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateOutcome {
    UpToDate,
    FastForwarded(String),
    Rebased(String),
    /// The rebase hit conflicts and was aborted; the worktree is exactly as before.
    Conflict(String),
}

/// Apply a detected remote-head change — explicitly, never automatically. A clean session
/// fast-forwards; local work rebases onto the new head; a conflicted rebase aborts whole.
pub fn update(worktree: &Path, provider: Provider, number: u64) -> Result<UpdateOutcome, String> {
    let new_head = fetch_review_head(worktree, provider, number)?;
    let current = git(worktree, &["rev-parse", "HEAD"])?;
    if current == new_head {
        return Ok(UpdateOutcome::UpToDate);
    }
    let clean = git(worktree, &["status", "--porcelain"])?.is_empty();
    let is_ancestor = git(worktree, &["merge-base", "--is-ancestor", "HEAD", &new_head]).is_ok();
    if clean && is_ancestor {
        git(worktree, &["merge", "--ff-only", &new_head])?;
        return Ok(UpdateOutcome::FastForwarded(new_head));
    }
    match git(worktree, &["rebase", &new_head]) {
        Ok(_) => Ok(UpdateOutcome::Rebased(new_head)),
        Err(error) => {
            let _ = git(worktree, &["rebase", "--abort"]);
            Ok(UpdateOutcome::Conflict(error))
        }
    }
}

/// Whether the deep worktree carries uncommitted edits.
pub fn dirty(worktree: &Path) -> bool {
    git(worktree, &["status", "--porcelain"]).is_ok_and(|out| !out.is_empty())
}

/// Commits on the review branch that exist nowhere but here (ahead of `base_sha`).
pub fn local_only_commits(worktree: &Path, base_sha: &str) -> u32 {
    git(worktree, &["rev-list", "--count", &format!("{base_sha}..HEAD")])
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

/// Delete a worktree this module created, plus its private branch. Never called for a
/// pre-existing local worktree — End Deep Review only removes what materialization made.
pub fn remove(repo: &Path, worktree: &Path, branch: &str) -> Result<(), String> {
    git(repo, &["worktree", "remove", "--force", &worktree.to_string_lossy()])?;
    let _ = git(repo, &["branch", "-D", branch]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An "origin" repo publishing review 5, and a clone that reviews it.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin");
        let clone = root.path().join("clone");
        let state = root.path().join("state");
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "-q", "-b", "main"]);
        ident(&origin);
        std::fs::write(origin.join("a.txt"), "one\ntwo\n").unwrap();
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-qm", "base"]);
        // The review head: a branch published under the PR ref.
        run(&origin, &["checkout", "-qb", "feature"]);
        std::fs::write(origin.join("a.txt"), "one\ntwo\nfeature\n").unwrap();
        run(&origin, &["commit", "-aqm", "feature work"]);
        run(&origin, &["update-ref", "refs/pull/5/head", "HEAD"]);
        run(&origin, &["checkout", "-q", "main"]);
        run(root.path(), &["clone", "-q", origin.to_str().unwrap(), clone.to_str().unwrap()]);
        ident(&clone);
        (root, origin, clone, state)
    }

    /// CI runners have no global git identity; the fixture repos carry their own.
    fn ident(repo: &Path) {
        run(repo, &["config", "user.email", "reviewr-test@example.com"]);
        run(repo, &["config", "user.name", "reviewr test"]);
    }

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn sha(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev]).unwrap()
    }

    #[test]
    fn materializes_the_exact_review_head_on_a_writable_branch() {
        let (_root, origin, clone, state) = setup();
        let m = materialize(&clone, &state, Provider::Github, 5, "github:x/o/r#5").unwrap();
        assert!(m.created);
        assert_eq!(m.branch, "reviewr/pr-5");
        assert_eq!(m.head, sha(&origin, "refs/pull/5/head"), "the exact provider head");
        assert_eq!(sha(&m.worktree, "HEAD"), m.head);
        assert!(m.worktree.starts_with(&state), "the worktree lives outside any reviewed checkout");
        // Writable: a local commit on the review branch succeeds.
        std::fs::write(m.worktree.join("note.txt"), "local\n").unwrap();
        run(&m.worktree, &["add", "-A"]);
        run(&m.worktree, &["commit", "-qm", "local note"]);
        assert_eq!(local_only_commits(&m.worktree, &m.head), 1);
    }

    #[test]
    fn a_second_materialization_reuses_the_worktree() {
        let (_root, _origin, clone, state) = setup();
        let first = materialize(&clone, &state, Provider::Github, 5, "k").unwrap();
        let second = materialize(&clone, &state, Provider::Github, 5, "k").unwrap();
        assert!(!second.created, "reinvocation resumes, never duplicates");
        assert_eq!(second.worktree, first.worktree);
        assert_eq!(second.head, first.head);
    }

    #[test]
    fn a_branch_name_collision_picks_the_next_free_suffix() {
        let (_root, _origin, clone, state) = setup();
        run(&clone, &["branch", "reviewr/pr-5"]);
        let m = materialize(&clone, &state, Provider::Github, 5, "k").unwrap();
        assert_eq!(m.branch, "reviewr/pr-5-1", "the stale branch is never reset");
    }

    #[test]
    fn a_clean_session_fast_forwards_on_explicit_update() {
        let (_root, origin, clone, state) = setup();
        let m = materialize(&clone, &state, Provider::Github, 5, "k").unwrap();
        // The remote head moves forward.
        run(&origin, &["checkout", "-q", "feature"]);
        std::fs::write(origin.join("a.txt"), "one\ntwo\nfeature\nmore\n").unwrap();
        run(&origin, &["commit", "-aqm", "more"]);
        run(&origin, &["update-ref", "refs/pull/5/head", "HEAD"]);
        run(&origin, &["checkout", "-q", "main"]);

        let outcome = update(&m.worktree, Provider::Github, 5).unwrap();
        let new = sha(&origin, "refs/pull/5/head");
        assert_eq!(outcome, UpdateOutcome::FastForwarded(new.clone()));
        assert_eq!(sha(&m.worktree, "HEAD"), new);
        assert_eq!(update(&m.worktree, Provider::Github, 5).unwrap(), UpdateOutcome::UpToDate);
    }

    #[test]
    fn local_commits_rebase_onto_the_new_head_and_conflicts_abort_whole() {
        let (_root, origin, clone, state) = setup();
        let m = materialize(&clone, &state, Provider::Github, 5, "k").unwrap();
        // Local review work.
        std::fs::write(m.worktree.join("review.txt"), "notes\n").unwrap();
        run(&m.worktree, &["add", "-A"]);
        run(&m.worktree, &["commit", "-qm", "review notes"]);
        // The remote head moves (compatible change).
        run(&origin, &["checkout", "-q", "feature"]);
        std::fs::write(origin.join("b.txt"), "new file\n").unwrap();
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-qm", "compatible"]);
        run(&origin, &["update-ref", "refs/pull/5/head", "HEAD"]);

        let outcome = update(&m.worktree, Provider::Github, 5).unwrap();
        assert!(matches!(outcome, UpdateOutcome::Rebased(_)), "{outcome:?}");
        assert_eq!(local_only_commits(&m.worktree, &sha(&origin, "refs/pull/5/head")), 1);

        // A conflicting remote change: the rebase aborts and the worktree stays put.
        let before = sha(&m.worktree, "HEAD");
        std::fs::write(origin.join("review.txt"), "conflicting remote\n").unwrap();
        run(&origin, &["add", "-A"]);
        run(&origin, &["commit", "-qm", "conflicts"]);
        run(&origin, &["update-ref", "refs/pull/5/head", "HEAD"]);
        run(&origin, &["checkout", "-q", "main"]);
        let outcome = update(&m.worktree, Provider::Github, 5).unwrap();
        assert!(matches!(outcome, UpdateOutcome::Conflict(_)), "{outcome:?}");
        assert_eq!(sha(&m.worktree, "HEAD"), before, "never left mid-rebase");
        assert!(!dirty(&m.worktree), "no conflict markers strand the worktree");
    }

    #[test]
    fn a_failed_creation_rolls_back_branch_and_directory() {
        let (_root, _origin, clone, state) = setup();
        // Sabotage: a plain file where the worktree directory must go.
        std::fs::create_dir_all(state.join("worktrees")).unwrap();
        std::fs::write(state.join("worktrees").join(key_hash("k")), "in the way").unwrap();
        let error = materialize(&clone, &state, Provider::Github, 5, "k").unwrap_err();
        assert!(!error.is_empty());
        assert!(
            git(&clone, &["rev-parse", "--verify", "--quiet", "reviewr/pr-5"]).is_err(),
            "no stray branch survives the rollback"
        );
    }

    #[test]
    fn removal_deletes_only_what_materialization_made() {
        let (_root, _origin, clone, state) = setup();
        let m = materialize(&clone, &state, Provider::Github, 5, "k").unwrap();
        remove(&clone, &m.worktree, &m.branch).unwrap();
        assert!(!m.worktree.exists());
        assert!(git(&clone, &["rev-parse", "--verify", "--quiet", &m.branch]).is_err());
        // The source clone itself is untouched.
        assert_eq!(git(&clone, &["status", "--porcelain"]).unwrap(), "");
    }
}
