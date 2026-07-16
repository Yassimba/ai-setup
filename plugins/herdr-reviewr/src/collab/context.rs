//! The normalized `ReviewContext`: one JSON shape for "where the reviewer is and what they
//! mean", independent of provider or scope.
//!
//! Every collaboration surface speaks this shape — prompt-context snapshots, tray resources,
//! and the target keys that bind a session, a Pi hello, and (later) a Deep Review workspace
//! to one review. UI commands and the extension consume this normalization instead of
//! branching on GitHub/GitLab/local anywhere downstream.

use serde_json::{Value, json};

use crate::forge;

/// The collaboration identity of one review target.
///
/// Remote reviews key by provider, host, repository, and number — the same review is the
/// same session no matter which worktree browses it. Local reviews key by canonical worktree,
/// with the scopes as switchable projections inside that one session.
pub fn remote_target_key(target: &crate::git::RepoTarget, number: u64) -> String {
    let provider = match target.provider {
        forge::Provider::Github => "github",
        forge::Provider::Gitlab => "gitlab",
    };
    format!(
        "{provider}:{}/{}{}{number}",
        target.host,
        target.full_path(),
        match target.provider {
            forge::Provider::Github => '#',
            forge::Provider::Gitlab => '!',
        }
    )
}

/// The collaboration identity of one local worktree. Canonicalized so a symlinked path and
/// its real path name the same session.
pub fn local_target_key(worktree: &std::path::Path) -> String {
    format!("local:{}", canonical_worktree_key(worktree))
}

/// The canonical identity string of one worktree, shared by the target key and the socket
/// hash on both sides of the collaboration link. Symlinks resolve so aliased paths name the
/// same session. On Windows, `canonicalize` returns verbatim `\\?\C:\...` paths while the
/// extension's `realpathSync` yields plain `C:\...`, and NTFS ignores case — so the verbatim
/// prefix is dropped and the path lowercased. Must match `canonicalWorktreeKey` in
/// pi-reviewr-collab's client.ts. Identity only — never use the result for filesystem access.
pub fn canonical_worktree_key(worktree: &std::path::Path) -> String {
    let canonical = worktree.canonicalize().unwrap_or_else(|_| worktree.to_path_buf());
    let display = canonical.display().to_string();
    if cfg!(windows) { windows_key(&display) } else { display }
}

/// The Windows normalization of [`canonical_worktree_key`], as a pure string function so
/// every platform's suite covers it: drop the verbatim prefix (rewriting `\\?\UNC\host\share`
/// back to `\\host\share`), then lowercase — Rust and Node may disagree on drive-letter or
/// on-disk casing.
fn windows_key(path: &str) -> String {
    let plain = match path.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => path.strip_prefix(r"\\?\").unwrap_or(path).to_string(),
    };
    plain.to_lowercase()
}

/// The Deep Review identity of one local worktree: the worktree plus its checkout. The
/// worktree alone says where the review happens, not what is being reviewed — keying the
/// session by `checkout` (the branch, or the commit when detached) parks each review with
/// its branch, so Shift+D on new work starts fresh instead of reviving another branch's
/// drafts and Pi conversation, and returning to a branch resumes exactly its session.
pub fn local_deep_target_key(worktree: &std::path::Path, checkout: &str) -> String {
    format!("{}@{checkout}", local_target_key(worktree))
}

/// One remote comment/discussion as a protocol resource: identity, anchor, root body, the
/// complete reply chain, and the patch evidence. This is what a tray alias resolves to and
/// what `item` carries in a snapshot — an alias never loses evidence to later navigation.
pub fn resource_json(comment: &forge::Comment) -> Value {
    json!({
        "kind": match comment.kind {
            forge::CommentKind::Review => "review",
            forge::CommentKind::Comment => "comment",
            forge::CommentKind::Finding => "finding",
        },
        "author": comment.author,
        "anchor": comment.anchor,
        "body": comment.body,
        "patch": comment.snippet,
        "resolved": comment.is_resolved,
        "outdated": comment.is_outdated,
        "replies_complete": comment.replies_state == forge::RepliesState::Complete,
        "replies": comment.replies.iter().map(|reply| json!({
            "author": reply.author,
            "body": reply.body,
            "created_at": reply.created_at,
        })).collect::<Vec<_>>(),
        "thread": comment.remote_id.as_ref().map(|id| id.thread_id.clone()),
    })
}

/// The stable in-session identity of one remote comment, for tray keys. Falls back to the
/// anchor + timestamp pair when a provider returned no id (it still distinguishes items).
pub fn resource_key(comment: &forge::Comment) -> String {
    comment.remote_id.as_ref().map_or_else(
        || format!("{}@{}", comment.anchor, comment.created_at),
        |id| id.thread_id.clone(),
    )
}

/// Where the reviewer's attention is, independent of the active tab's projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub side: crate::model::Side,
    /// The selected range end line (the anchor line for a single-line location).
    pub line: u32,
    /// The range start when a multi-line selection is active.
    pub start_line: Option<u32>,
}

/// Everything one prompt-context snapshot carries, gathered from a single viewer-state read.
#[derive(Debug)]
pub struct Snapshot {
    /// The review identity the prompt is about (a remote key or a local key).
    pub target: String,
    /// The concrete source behind the target: `github-pr`, `gitlab-mr`, `uncommitted`,
    /// `branch`, or `last-turn`.
    pub source: String,
    pub worktree: std::path::PathBuf,
    pub location: Option<Location>,
    /// The visible hunk (or file excerpt) around the location, so "this deletion" can be
    /// reasoned about without reconstructing the view.
    pub patch: Option<String>,
    /// The selected comment or discussion, as a [`resource_json`] value.
    pub item: Option<Value>,
    /// The tray, as [`super::session::CollaborationSession::tray_json`] produced it.
    pub tray: Value,
}

impl Snapshot {
    /// The protocol encoding carried inside a `context` frame.
    pub fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "source": self.source,
            "worktree": self.worktree.display().to_string(),
            "location": self.location.as_ref().map(|l| json!({
                "path": l.path,
                "side": match l.side {
                    crate::model::Side::New => "new",
                    crate::model::Side::Old => "old",
                },
                "line": l.line,
                "start_line": l.start_line,
            })),
            "patch": self.patch,
            "item": self.item,
            "tray": self.tray,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_keys_name_provider_host_path_and_number() {
        let github = crate::git::RepoTarget {
            provider: forge::Provider::Github,
            host: "github.com".into(),
            owner: "acme".into(),
            name: "widgets".into(),
        };
        assert_eq!(remote_target_key(&github, 7), "github:github.com/acme/widgets#7");
        let gitlab = crate::git::RepoTarget {
            provider: forge::Provider::Gitlab,
            host: "git.example.com".into(),
            owner: "group/sub".into(),
            name: "proj".into(),
        };
        assert_eq!(remote_target_key(&gitlab, 12), "gitlab:git.example.com/group/sub/proj!12");
    }

    #[test]
    fn local_keys_canonicalize_so_symlinked_paths_share_a_session() {
        let dir = tempfile::tempdir().unwrap();
        // The tempdir path and its canonicalized form differ on macOS (/tmp is a symlink)
        // and on Windows (verbatim prefix, casing); both must yield one session key.
        let real = dir.path().canonicalize().unwrap();
        assert_eq!(local_target_key(dir.path()), local_target_key(&real));
        // A path that cannot canonicalize (not yet created) still keys deterministically,
        // through the same platform normalization as canonicalizable paths.
        let ghost = dir.path().join("missing");
        let ghost_display = ghost.display().to_string();
        let expected = if cfg!(windows) { windows_key(&ghost_display) } else { ghost_display };
        assert_eq!(local_target_key(&ghost), format!("local:{expected}"));
    }

    #[test]
    fn windows_keys_drop_the_verbatim_prefix_and_case() {
        assert_eq!(windows_key(r"\\?\C:\Users\Jan Dirk\repo"), r"c:\users\jan dirk\repo");
        assert_eq!(windows_key(r"\\?\UNC\host\share\repo"), r"\\host\share\repo");
        assert_eq!(windows_key(r"C:\Users\Jan Dirk\repo"), r"c:\users\jan dirk\repo");
        assert_eq!(windows_key(r"c:\already\lower"), r"c:\already\lower");
    }

    #[test]
    fn key_hash_vectors_match_the_pi_extension() {
        // Twin: pi-reviewr-collab/test/client.test.ts "windows-normalized hash vectors match
        // reviewr's Rust side" — the same inputs must hash identically there, or the two
        // sides derive different socket names and target keys and never meet.
        let hash = crate::collab::materialize::key_hash;
        assert_eq!(hash(r"alice|c:\users\jan dirk\repo"), "a111569fc1f3afa1");
        assert_eq!(
            hash(&format!("alice|{}", windows_key(r"\\?\C:\Users\Jan Dirk\repo"))),
            "a111569fc1f3afa1"
        );
    }

    #[test]
    fn resources_carry_identity_evidence_and_reply_completeness() {
        let comment = forge::Comment {
            kind: forge::CommentKind::Finding,
            author: "rev".into(),
            author_is_bot: false,
            anchor: "src/a.rs:9".into(),
            body: "boundary".into(),
            snippet: Some("@@ hunk".into()),
            created_at: "2026-07-01T00:00:00Z".into(),
            is_resolved: false,
            is_outdated: true,
            reply_count: 1,
            replies: vec![forge::RemoteReply {
                id: "r1".into(),
                author: "alice".into(),
                body: "agreed".into(),
                created_at: "2026-07-01T01:00:00Z".into(),
            }],
            replies_state: forge::RepliesState::Partial {
                missing: 2,
                reason: forge::PartialReason::Capped,
            },
            diff_anchor: None,
            remote_id: Some(forge::RemoteCommentId {
                thread_id: "T1".into(),
                root_comment_id: Some(5),
            }),
        };
        let resource = resource_json(&comment);
        assert_eq!(resource["kind"], "finding");
        assert_eq!(resource["anchor"], "src/a.rs:9");
        assert_eq!(resource["patch"], "@@ hunk");
        assert_eq!(resource["outdated"], true);
        assert_eq!(resource["replies"][0]["author"], "alice");
        assert_eq!(
            resource["replies_complete"], false,
            "a partial chain is declared, so the agent never reasons over a silent prefix"
        );
        assert_eq!(resource_key(&comment), "T1");
        // Without a provider id the key still distinguishes items.
        let mut bare = comment;
        bare.remote_id = None;
        assert_eq!(resource_key(&bare), "src/a.rs:9@2026-07-01T00:00:00Z");
    }

    #[test]
    fn snapshots_encode_location_sides_and_ride_the_tray_verbatim() {
        let snapshot = Snapshot {
            target: "github:github.com/acme/widgets#7".into(),
            source: "github-pr".into(),
            worktree: std::path::PathBuf::from("/work/tree"),
            location: Some(Location {
                path: "src/a.rs".into(),
                side: crate::model::Side::Old,
                line: 9,
                start_line: Some(4),
            }),
            patch: Some("@@ -4,6 +4,6 @@".into()),
            item: None,
            tray: json!([{"alias": "C1", "body": "evidence"}]),
        };
        let v = snapshot.to_json();
        assert_eq!(v["target"], "github:github.com/acme/widgets#7");
        assert_eq!(v["location"]["side"], "old");
        assert_eq!(v["location"]["start_line"].as_u64(), Some(4));
        assert_eq!(v["tray"][0]["alias"], "C1");
        assert_eq!(v["item"], Value::Null, "no selected item reads as null, not missing");

        let bare = Snapshot {
            target: "local:/work/tree".into(),
            source: "uncommitted".into(),
            worktree: std::path::PathBuf::from("/work/tree"),
            location: None,
            patch: None,
            item: None,
            tray: json!([]),
        };
        assert_eq!(bare.to_json()["location"], Value::Null);
    }
}
