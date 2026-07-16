//! Read-only forge access: the pull/merge request's identity, state, checks, and comments.
//!
//! See `specs/forge-host.md`. A fetch first derives [`PrFetchInput`] from local Git and one
//! validated config snapshot, then reads its canonical target through the provider that owns
//! the origin host — GitHub via `gh` ([`github`]) or GitLab via `glab` ([`gitlab`]). It never
//! reads never write; the only write path is an explicit grouped [`sync_review`] request. The
//! `PR` tab renders the [`PrSnapshot`] this module produces; degradation is in-band as [`PrView`].

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod detect;
pub(crate) mod github;
pub(crate) mod gitlab;

/// Which forge a repository target belongs to — decides the CLI, the query dialect, and the
/// user-facing labels (PR vs MR).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Github,
    Gitlab,
}

impl Provider {
    /// The short label for the unit of review: GitHub pull request, GitLab merge request.
    #[must_use]
    pub fn unit(self) -> &'static str {
        match self {
            Self::Github => "PR",
            Self::Gitlab => "MR",
        }
    }

    /// The number sigil the forge itself uses: `#42` on GitHub, `!42` on GitLab.
    #[must_use]
    pub fn number_prefix(self) -> &'static str {
        match self {
            Self::Github => "#",
            Self::Gitlab => "!",
        }
    }

    /// The forge's display name, for link and empty-state wording.
    #[must_use]
    pub fn forge_name(self) -> &'static str {
        match self {
            Self::Github => "GitHub",
            Self::Gitlab => "GitLab",
        }
    }
}

/// What the `PR` tab shows: the resolved snapshot, or a degraded state with its own remedy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrView {
    /// Work is pending but has not crossed the loading-indicator delay.
    Pending,
    /// Work crossed the loading-indicator delay without producing a snapshot.
    Loading,
    /// An open (or merged/closed) PR/MR resolved for one of the worktree's candidate branches.
    Pr(Box<PrSnapshot>),
    /// No candidate branch has a PR/MR; the queried candidate names, so the empty state can
    /// say what was looked for. Empty on a detached `HEAD` (nothing was queried).
    NoPr(Vec<String>),
    /// Two or more open PRs/MRs back the winning candidate branch and not exactly one matches
    /// the pinned `HEAD`; the count, so the user knows to pick on the forge.
    Ambiguous(usize),
    /// The provider's CLI (`gh` / `glab`) is not on `PATH`; carries the tool name.
    NoCli(&'static str),
    /// The CLI is installed but not authenticated for this canonical host.
    NotAuthed { tool: &'static str, host: String },
    /// Origin is missing or has no hosted Git URL.
    NeedsSupportedOrigin,
    /// Origin names a hosted forge outside the supported hosts (no CLI claims it either).
    UnsupportedHost(String),
    /// Origin names a supported host but not a repository path.
    MalformedOrigin(String),
    /// The host is known and authenticated but the API cannot be reached (VPN wall,
    /// IP allowlist, DNS, connection refused); the app freezes the last good view.
    ApiUnreachable { host: String, detail: String },
    /// Any other CLI failure (rate limit, …); the app freezes the last good view.
    Error(String),
}

impl PrView {
    /// A same-input failure that can be retried without discarding the visible snapshot.
    /// Both snapshot preservation and the empty-state renderer consume this projection so a
    /// newly added retryable failure cannot diverge between those surfaces.
    pub fn retry_remedy(&self) -> Option<String> {
        match self {
            Self::NoCli(tool) => Some(format!("{tool} not found — install `{tool}`, then press r")),
            Self::NotAuthed { tool, host } => Some(format!(
                "not signed in — run `{tool} auth login --hostname {host}`, then press r"
            )),
            Self::ApiUnreachable { host, detail } => {
                Some(format!("{host} unreachable — {detail}; check VPN/proxy, then press r"))
            }
            Self::Error(message) => {
                Some(format!("forge unavailable — {message}; press r to retry now"))
            }
            _ => None,
        }
    }
}

/// One pull/merge request's state, read fresh from the forge each poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrSnapshot {
    /// The forge that produced this snapshot — drives PR/MR labels and link wording.
    pub provider: Provider,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub is_draft: bool,
    /// The PR's head branch name — the candidate that resolved, which may differ from the
    /// worktree's local branch name (`specs/forge-host.md`).
    pub head_ref: String,
    /// The head branch lives in another repository (GitHub `isCrossRepository`, GitLab
    /// cross-project MR); shown as a marker so a same-named fork PR is visible.
    pub head_is_fork: bool,
    pub base_ref: String,
    /// Commit identities required to anchor new inline comments on the selected review.
    pub diff_refs: DiffRefs,
    pub merge: Merge,
    pub sync: Sync,
    pub checks: Vec<Check>,
    pub comments: Vec<Comment>,
    /// A capped surface (reviews/comments/threads/checks) still had more rows after the
    /// fetch's page walk — the lists shown are a prefix, not the whole set. Drives a
    /// "more on <forge>" marker.
    pub truncated: bool,
    /// The discussion/thread list itself is an explicitly marked prefix: following its
    /// cursor chain failed or ran out of budget. `None` when every thread was listed.
    pub threads_partial: Option<PartialReason>,
}

/// The PR/MR lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

/// Whether the PR/MR has a merge blocker worth surfacing. Only the actionable blockers are
/// modelled; states that carry nothing a reviewer acts on fold into `Clean`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    Clean,
    Conflicting,
    Blocked,
}

/// The local branch's position relative to the PR/MR head (`head_oid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    InSync,
    /// Local `HEAD` is ahead of the PR head by N commits — the PR lags your local tree.
    Unpushed(u32),
    /// The PR head is ahead of local `HEAD` by N commits.
    Behind(u32),
    /// The PR head object is not available locally, so its relation to `HEAD` is unknowable.
    Unknown,
}

/// One CI check, the latest run for its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
}

/// A check's outcome, normalised across providers (GitHub check runs and commit statuses,
/// GitLab pipeline jobs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Success,
    Failure,
    Running,
    Pending,
    Skipped,
}

/// Provider-neutral commit identities for a review diff. GitHub uses `head_sha` when creating
/// a grouped review; GitLab requires the complete base/start/head triple for positions.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct DiffRefs {
    pub base_sha: String,
    pub start_sha: String,
    pub head_sha: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffAnchor {
    pub path: String,
    pub old_path: Option<String>,
    pub side: crate::model::Side,
    pub line: u32,
    pub start_line: Option<u32>,
    /// The `(start, end)` diff-parser positions of the anchored range, computed from the local
    /// patch when a draft is authored. GitLab's multi-line positions need both counters per
    /// endpoint (its line codes are `sha1(path)_<old>_<new>`); GitHub ignores this, and anchors
    /// read back from a forge leave it `None`.
    pub endpoints: Option<(crate::diff::RangeEndpoint, crate::diff::RangeEndpoint)>,
}

impl DiffAnchor {
    /// The `path:line` (or `path:start-end` for a ranged anchor) label.
    #[must_use]
    pub fn location(&self) -> String {
        match self.start_line {
            Some(start) if start != self.line => format!("{}:{start}-{}", self.path, self.line),
            _ => format!("{}:{}", self.path, self.line),
        }
    }
}

/// Stable provider identifiers needed to reply to an existing comment/discussion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCommentId {
    pub thread_id: String,
    pub root_comment_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteReply {
    /// The provider's node identity, used to drop duplicate rows when merging follow-up
    /// pages; empty when a provider surface carries none.
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// Why a reply chain or thread list is a prefix of what the forge holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartialReason {
    /// A follow-up page request failed mid-walk; what had already arrived is shown.
    PageFailed(String),
    /// The page-follow budget ran out before the forge ran out of pages.
    Capped,
}

/// Whether every reply the forge holds for one discussion made it into [`Comment::replies`].
/// Completeness is decided by walking pages to their end — never inferred from a capped
/// first page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RepliesState {
    #[default]
    Complete,
    /// The shown replies are a prefix; `missing` counts the replies that did not load.
    Partial { missing: u32, reason: PartialReason },
}

/// One incoming comment: a PR-level review, a plain comment, or an inline finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub kind: CommentKind,
    pub author: String,
    pub author_is_bot: bool,
    /// `path:line` for a finding, the literal `review`/`comment` for the unanchored kinds.
    pub anchor: String,
    pub body: String,
    /// The finding's diff hunk as the forge returns it; `None` for a review or comment.
    pub snippet: Option<String>,
    /// The post time as the forge's ISO-8601 string (`…Z`), the newest-first sort key.
    pub created_at: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub reply_count: u32,
    pub replies: Vec<RemoteReply>,
    /// Whether `replies` is the complete chain or an explicitly marked prefix.
    pub replies_state: RepliesState,
    pub diff_anchor: Option<DiffAnchor>,
    pub remote_id: Option<RemoteCommentId>,
}

/// What a comment is anchored to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    Review,
    Comment,
    Finding,
}

impl PrSnapshot {
    /// The overall check rollup: any failure fails, else any still-running is running, else success.
    /// `None` when the PR has no checks.
    #[must_use]
    pub fn checks_rollup(&self) -> Option<CheckStatus> {
        if self.checks.is_empty() {
            return None;
        }
        if self.checks.iter().any(|c| c.status == CheckStatus::Failure) {
            return Some(CheckStatus::Failure);
        }
        if self
            .checks
            .iter()
            .any(|c| matches!(c.status, CheckStatus::Running | CheckStatus::Pending))
        {
            return Some(CheckStatus::Running);
        }
        Some(CheckStatus::Success)
    }

    /// How many checks have failed — the count behind the `✗ N failing` rollup label.
    #[must_use]
    pub fn failing_checks(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Failure).count()
    }
}

/// A classified CLI failure, mapped to a [`PrView`] degraded state. Shared by both providers;
/// `tool` names the CLI that failed so remedies stay copy-pasteable. The two resolution
/// outcomes (`Ambiguous`, `NoPr`) ride the same channel so a provider's resolve step can
/// abort the fetch with a first-class view instead of a second plumbing type.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliError {
    NoCli(&'static str),
    NotAuthed { tool: &'static str, host: String },
    Unreachable { host: String, detail: String },
    Other(String),
    Ambiguous(usize),
    NoPr(Vec<String>),
}

impl From<CliError> for PrView {
    fn from(e: CliError) -> Self {
        match e {
            CliError::NoCli(tool) => PrView::NoCli(tool),
            CliError::NotAuthed { tool, host } => PrView::NotAuthed { tool, host },
            CliError::Unreachable { host, detail } => PrView::ApiUnreachable { host, detail },
            CliError::Other(m) => PrView::Error(m),
            CliError::Ambiguous(count) => PrView::Ambiguous(count),
            CliError::NoPr(candidates) => PrView::NoPr(candidates),
        }
    }
}

/// Every local and configuration value that identifies one PR fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrFetchInput {
    pub origin: crate::git::OriginIdentity,
    pub branch: Option<String>,
    pub head_oid: Option<String>,
    pub candidates: Vec<String>,
    pub base: Option<String>,
    pub base_branches: Vec<String>,
    /// A picker-pinned PR/MR number: the fetch skips branch resolution and reads this one
    /// directly. Participates in input equality, so pinning and unpinning refetch naturally.
    pub pinned: Option<u64>,
}

/// One row of the picker: a PR/MR a user can pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrListItem {
    pub number: u64,
    pub title: String,
    pub head_ref: String,
    pub author: String,
    pub is_draft: bool,
    pub state: PrState,
    /// The head pipeline / check-rollup summary, when the forge reports one — the closed
    /// section's "merged · passed / failed" suffix.
    pub ci: Option<CheckStatus>,
    /// ISO-8601 creation time: the newest-first sort key across the merged+closed aliases.
    pub created_at: String,
    /// Total human comments (GitLab `userNotesCount`, GitHub `totalCommentsCount`).
    pub comments: u32,
    /// Unresolved and resolved discussion counts, when the forge reports them (GitLab only —
    /// GitHub has no cheap per-row thread verdict).
    pub threads_open: Option<u32>,
    pub threads_resolved: Option<u32>,
}

/// The picker's two sections: open PRs/MRs, then merged and closed ones, each newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrListing {
    pub open: Vec<PrListItem>,
    pub done: Vec<PrListItem>,
}

/// Identity of one explicitly selected review diff. Results are tagged with this value so a
/// slow request cannot paint after the user pins another review or switches projects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDiffRequest {
    pub target: crate::git::RepoTarget,
    pub number: u64,
    request_id: u64,
}

static NEXT_REVIEW_DIFF_REQUEST: AtomicU64 = AtomicU64::new(1);

impl ReviewDiffRequest {
    /// Create a uniquely tagged request. Re-fetching the same review gets a new identity, so an
    /// older completion cannot overwrite fresher data.
    pub fn new(target: crate::git::RepoTarget, number: u64) -> Self {
        Self {
            target,
            number,
            request_id: NEXT_REVIEW_DIFF_REQUEST.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewDraftAction {
    Inline(DiffAnchor),
    Reply { remote_id: Option<RemoteCommentId>, author: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDraft {
    pub local_id: u64,
    pub action: ReviewDraftAction,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSyncRequest {
    pub target: crate::git::RepoTarget,
    pub number: u64,
    pub diff_refs: DiffRefs,
    pub drafts: Vec<ReviewDraft>,
    request_id: u64,
}

impl ReviewSyncRequest {
    pub fn new(
        target: crate::git::RepoTarget,
        number: u64,
        diff_refs: DiffRefs,
        drafts: Vec<ReviewDraft>,
    ) -> Self {
        Self {
            target,
            number,
            diff_refs,
            drafts,
            request_id: NEXT_REVIEW_DIFF_REQUEST.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct ReviewSyncOutcome {
    pub succeeded: Vec<u64>,
    pub failed: Vec<(u64, String)>,
    /// The request may have reached the provider but its response was lost. These items must not
    /// be retried blindly because doing so can duplicate a review comment.
    pub uncertain: Vec<(u64, String)>,
}

/// Submit one explicit draft group. Providers preserve the group as closely as their APIs
/// allow; per-item outcomes prevent retries from duplicating already accepted comments.
pub fn sync_review(repo: &Path, request: &ReviewSyncRequest) -> ReviewSyncOutcome {
    let cancelled = AtomicBool::new(false);
    match request.target.provider {
        Provider::Github => github::sync_review(repo, request, &cancelled),
        Provider::Gitlab => gitlab::sync_review(repo, request, &cancelled),
    }
}

/// Fetch a selected PR/MR's changed files and unified hunk bodies without fetching refs or
/// touching the checkout. Provider APIs may omit large/binary patches; that is represented in
/// each [`crate::diff::PatchFile`] rather than treated as a whole-request failure.
pub fn fetch_review_diff(
    repo: &Path,
    request: &ReviewDiffRequest,
) -> Result<crate::diff::PatchSet, String> {
    let cancelled = AtomicBool::new(false);
    let result = match request.target.provider {
        Provider::Github => {
            github::fetch_review_diff(repo, &request.target, request.number, &cancelled)
        }
        Provider::Gitlab => {
            gitlab::fetch_review_diff(repo, &request.target, request.number, &cancelled)
        }
    };
    result.map_err(|error| {
        let view = PrView::from(error);
        view.retry_remedy().unwrap_or_else(|| match view {
            PrView::Error(message) => message,
            _ => "remote diff unavailable".to_string(),
        })
    })
}

/// List the target's PRs/MRs for the picker, provider-dispatched. The error is the same
/// remedy wording the PR tab's degraded states use.
pub fn list_prs(repo: &Path, target: &crate::git::RepoTarget) -> Result<PrListing, String> {
    let cancelled = AtomicBool::new(false);
    let result = match target.provider {
        Provider::Github => github::list_prs(repo, target, &cancelled),
        Provider::Gitlab => gitlab::list_prs(repo, target, &cancelled),
    };
    result.map_err(|error| {
        PrView::from(error).retry_remedy().unwrap_or_else(|| "listing failed".to_string())
    })
}

/// Derive one complete fetch input without contacting the forge.
pub fn fetch_input(
    repo: &Path,
    base: Option<&str>,
    config: &crate::config::PluginConfig,
) -> Result<PrFetchInput, String> {
    let classifier = detect::HostClassifier::load(config);
    let local = crate::git::pr_local(repo, base, config.base_branches(), &classifier)
        .map_err(|error| error.0)?;
    Ok(PrFetchInput {
        origin: local.origin,
        branch: local.branch,
        head_oid: local.head_oid,
        candidates: local.candidates,
        base: base.map(str::to_owned),
        base_branches: config.base_branches().to_vec(),
        pinned: None,
    })
}

/// Read the forge for one already-derived input. Degradation stays in-band for the PR tab.
#[must_use]
pub fn fetch(repo: &Path, input: &PrFetchInput) -> PrView {
    fetch_cancellable(repo, input, &AtomicBool::new(false))
}

/// Read the forge with a cancellation signal owned by the event-loop coordinator. The origin
/// identity picks the provider; everything after the dispatch is provider-owned.
#[must_use]
pub(crate) fn fetch_cancellable(
    repo: &Path,
    input: &PrFetchInput,
    cancelled: &AtomicBool,
) -> PrView {
    let target = match &input.origin {
        crate::git::OriginIdentity::Repository(target) => target,
        crate::git::OriginIdentity::Missing | crate::git::OriginIdentity::Hostless => {
            return PrView::NeedsSupportedOrigin;
        }
        crate::git::OriginIdentity::Unsupported(host) => {
            return PrView::UnsupportedHost(host.clone());
        }
        crate::git::OriginIdentity::Malformed(host) => {
            return PrView::MalformedOrigin(host.clone());
        }
    };
    if input.candidates.is_empty() && input.pinned.is_none() {
        // A detached HEAD (e.g. after `gh pr merge --delete-branch`) has no branch identity
        // to publish, so nothing was derived. Show the empty state rather than querying an
        // empty head filter, which the forges treat as unfiltered. A pinned number needs no
        // branch identity and proceeds.
        return PrView::NoPr(Vec::new());
    }
    match target.provider {
        Provider::Github => github::fetch(repo, target, input, cancelled),
        Provider::Gitlab => gitlab::fetch(repo, target, input, cancelled),
    }
}

/// Run one explicitly targeted CLI invocation in `repo` and return stdout; a spawn miss is
/// `NoCli`, any other failure surfaces the child's stderr for the provider to classify.
/// Both pipes drain on threads while polling so a large GraphQL response cannot fill a pipe
/// and block the child before it exits. A superseded config/fetch kills the process; the
/// coordinator keeps ownership until this worker reports completion, preserving one real
/// fetch in flight.
pub(crate) fn run_tool(
    tool: &'static str,
    repo: &Path,
    args: &[&str],
    cancelled: &AtomicBool,
) -> Result<String, ToolFailure> {
    run_tool_input(tool, repo, args, None, cancelled)
}

/// [`run_tool`] with an optional UTF-8 stdin body, used for provider write APIs whose nested
/// JSON cannot be represented safely as CLI `--field` arguments.
pub(crate) fn run_tool_input(
    tool: &'static str,
    repo: &Path,
    args: &[&str],
    input: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<String, ToolFailure> {
    let child = Command::new(tool)
        .current_dir(repo)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolFailure::NoCli(tool));
        }
        Err(e) => return Err(ToolFailure::Io(e.to_string())),
    };

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("piped stdin");
        if let Err(error) = stdin.write_all(input.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolFailure::Io(error.to_string()));
        }
    }
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ToolFailure::Io(error.to_string()));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if cancelled.load(Ordering::Acquire) {
        return Err(ToolFailure::Io("request cancelled".to_string()));
    }
    if status.success() {
        return Ok(String::from_utf8_lossy(&stdout).into_owned());
    }
    Err(ToolFailure::Stderr(String::from_utf8_lossy(&stderr).into_owned()))
}

/// The `api graphql` argument vector, host pinned. Every variable is passed with `-f` (raw
/// string) — `-F` type-coerces, so a branch literally named `123` would arrive as an Int and
/// fail its `String!` declaration.
pub(crate) fn graphql_args(host: &str, query: &str, vars: &[(String, String)]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "api".to_string(),
        "graphql".to_string(),
        "--hostname".to_string(),
        host.to_owned(),
        "-f".to_string(),
        format!("query={query}"),
    ];
    for (key, value) in vars {
        args.push("-f".to_string());
        args.push(format!("{key}={value}"));
    }
    args
}

/// Run one GraphQL query through a provider's CLI and parse the JSON response. `classify`
/// maps the CLI's stderr wording to a degraded state — the only provider-specific part of
/// the exchange.
pub(crate) fn graphql(
    tool: &'static str,
    classify: fn(&str, &str) -> CliError,
    repo: &Path,
    host: &str,
    query: &str,
    vars: &[(String, String)],
    cancelled: &AtomicBool,
) -> Result<serde_json::Value, CliError> {
    let args = graphql_args(host, query, vars);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = match run_tool(tool, repo, &arg_refs, cancelled) {
        Ok(stdout) => stdout,
        Err(ToolFailure::NoCli(tool)) => return Err(CliError::NoCli(tool)),
        Err(ToolFailure::Io(detail)) => return Err(CliError::Other(detail)),
        Err(ToolFailure::Stderr(stderr)) => return Err(classify(&stderr, host)),
    };
    serde_json::from_str(&out).map_err(|e| CliError::Other(e.to_string()))
}

/// Whether a CLI's stderr describes a network wall (VPN, IP allowlist, DNS, proxy down)
/// rather than a forge-side failure — shared wording across `gh` and `glab`, so the two
/// classifiers cannot drift apart.
pub(crate) fn unreachable_stderr(lower: &str) -> bool {
    lower.contains("could not resolve host")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("error connecting")
        || lower.contains("timeout")
}

/// Convert a write failure into user text plus whether provider acceptance is unknowable.
pub(crate) fn sync_failure(error: CliError, fallback: &str) -> (String, bool) {
    let uncertain = match &error {
        CliError::Unreachable { .. } => true,
        CliError::Other(message) => {
            let lower = message.to_ascii_lowercase();
            !has_http_client_error(&lower)
                && !["validation", "not found"].iter().any(|needle| lower.contains(needle))
        }
        CliError::NoCli(_)
        | CliError::NotAuthed { .. }
        | CliError::Ambiguous(_)
        | CliError::NoPr(_) => false,
    };
    // Read remedies say "press r", but `r` only refreshes and must never imply a forge write
    // retry. Write failures stay attached to their draft and are retried only by explicit `s`.
    let message = match error {
        CliError::NoCli(tool) => format!("{tool} not found"),
        CliError::NotAuthed { tool, host } => {
            format!("not signed in — run `{tool} auth login --hostname {host}`")
        }
        CliError::Unreachable { host, detail } => format!("{host} unreachable — {detail}"),
        CliError::Other(message) => message,
        CliError::Ambiguous(_) | CliError::NoPr(_) => fallback.to_string(),
    };
    (message, uncertain)
}

fn has_http_client_error(message: &str) -> bool {
    ["http ", "status code "].iter().any(|marker| {
        message.match_indices(marker).any(|(index, _)| {
            message[index + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u16>()
                .is_ok_and(|status| (400..500).contains(&status))
        })
    })
}

/// How a CLI invocation failed, before provider-specific stderr classification.
pub(crate) enum ToolFailure {
    NoCli(&'static str),
    Io(String),
    Stderr(String),
}

/// How many follow-up pages one cursored surface may fetch before the walk stops with an
/// explicit [`PartialReason::Capped`] — 20 pages of 100 rows on top of the first page.
pub(crate) const PAGE_BUDGET: u32 = 20;

/// One follow-up page of a cursored connection: its rows and where the walk goes next.
pub(crate) struct Page {
    pub nodes: Vec<serde_json::Value>,
    pub has_next: bool,
    pub cursor: Option<String>,
}

/// Follow a connection's cursor chain to its end, within `budget` pages. `start` is the
/// first page's end cursor — `None` when the surface is already complete. Returns every
/// follow-up node plus why the walk stopped early, if it did; a `None` reason means the
/// chain was walked to its end.
pub(crate) fn follow_pages(
    start: Option<String>,
    budget: u32,
    mut fetch: impl FnMut(&str) -> Result<Page, CliError>,
) -> (Vec<serde_json::Value>, Option<PartialReason>) {
    let mut nodes = Vec::new();
    let Some(mut cursor) = start else { return (nodes, None) };
    for _ in 0..budget {
        match fetch(&cursor) {
            Ok(page) => {
                nodes.extend(page.nodes);
                if !page.has_next {
                    return (nodes, None);
                }
                match page.cursor {
                    Some(next) => cursor = next,
                    // A next page without a cursor cannot be walked; an honest cap beats
                    // refetching the same page forever.
                    None => return (nodes, Some(PartialReason::Capped)),
                }
            }
            Err(error) => {
                return (nodes, Some(PartialReason::PageFailed(page_error(error))));
            }
        }
    }
    (nodes, Some(PartialReason::Capped))
}

/// A page-walk failure as user text, reusing the degraded-state remedies so wording cannot
/// drift from the PR tab's.
fn page_error(error: CliError) -> String {
    let view = PrView::from(error);
    view.retry_remedy().unwrap_or_else(|| match view {
        PrView::Error(message) => message,
        _ => "page fetch failed".to_string(),
    })
}

/// Append follow-up page nodes onto a connection's `nodes` array, dropping rows whose key
/// already appears — a shifting list may repeat rows across page requests.
pub(crate) fn append_nodes_by_key(
    connection: &mut serde_json::Value,
    extra: Vec<serde_json::Value>,
    key: impl Fn(&serde_json::Value) -> Option<String>,
) {
    let Some(dest) = connection["nodes"].as_array_mut() else { return };
    let mut seen: std::collections::HashSet<String> = dest.iter().filter_map(&key).collect();
    for node in extra {
        if let Some(id) = key(&node)
            && !seen.insert(id)
        {
            continue;
        }
        dest.push(node);
    }
}

/// [`append_nodes_by_key`] keyed by the provider's `id` field.
pub(crate) fn append_nodes(connection: &mut serde_json::Value, extra: Vec<serde_json::Value>) {
    append_nodes_by_key(connection, extra, |node| node["id"].as_str().map(str::to_owned));
}

/// Chronological thread order for a merged reply chain; ISO-8601 `…Z` strings sort lexically
/// and the stable sort keeps arrival order on ties.
pub(crate) fn sort_replies(replies: &mut [RemoteReply]) {
    replies.sort_by(|a, b| a.created_at.cmp(&b.created_at));
}

/// Keep only the latest PR-level (`review`/`comment`) post per bot author; humans keep all.
pub(crate) fn dedup_bot_prose(out: &mut Vec<Comment>) {
    let mut keep_newest: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for c in out.iter() {
        if c.author_is_bot && c.kind != CommentKind::Finding {
            let e = keep_newest.entry(c.author.clone()).or_default();
            if c.created_at > *e {
                e.clone_from(&c.created_at);
            }
        }
    }
    out.retain(|c| {
        !(c.author_is_bot && c.kind != CommentKind::Finding)
            || keep_newest.get(&c.author) == Some(&c.created_at)
    });
}

/// The winner among the candidates' open PRs (`specs/forge-host.md` "Resolution").
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    One(u64),
    Ambiguous(usize),
    None,
}

/// Pick the open PR: the earliest candidate in derivation order holding any wins — the
/// recorded upstream outranks an inferred branch, which outranks the bare local name. On
/// one name backing several open PRs, exactly one head at the pinned `HEAD` wins; else the
/// ambiguity count is surfaced rather than a silent guess.
pub(crate) fn select_open(per_candidate: &[Vec<(u64, String)>], pinned_head: Option<&str>) -> Pick {
    for prs in per_candidate {
        match prs.as_slice() {
            [] => {}
            [(number, _)] => return Pick::One(*number),
            many => {
                if let Some(pin) = pinned_head {
                    let mut hits = many.iter().filter(|(_, oid)| oid == pin);
                    if let (Some((number, _)), None) = (hits.next(), hits.next()) {
                        return Pick::One(*number);
                    }
                }
                return Pick::Ambiguous(many.len());
            }
        }
    }
    Pick::None
}

/// The historical fallback: the newest-created merged/closed PR across all candidates.
/// ISO-8601 `…Z` strings compare lexically; a strict `>` keeps the earlier candidate on a
/// timestamp tie, so the pick is deterministic.
pub(crate) fn select_historical(per_candidate: &[Vec<(u64, String)>]) -> Option<u64> {
    let mut best: Option<(u64, &str)> = None;
    for prs in per_candidate {
        for (number, created) in prs {
            if best.is_none_or(|(_, b)| created.as_str() > b) {
                best = Some((*number, created));
            }
        }
    }
    best.map(|(number, _)| number)
}

/// The local branch's position relative to the PR head, from `git`'s ahead/behind counts. A
/// diverged branch (both nonzero) leads with the unpushed count — the headline case. `None`
/// (the PR head isn't local yet) stays explicitly unknown rather than guessing.
pub(crate) fn derive_sync(ahead_behind: Option<(u32, u32)>) -> Sync {
    match ahead_behind {
        None => Sync::Unknown,
        Some((0, 0)) => Sync::InSync,
        Some((0, behind)) => Sync::Behind(behind),
        Some((ahead, _)) => Sync::Unpushed(ahead),
    }
}

/// A relative age label (`5m`, `2h`, `3d`, `2w`) from an ISO-8601 `…Z` timestamp, against `now`.
/// `now` is injected so the formatting is testable; the UI passes `SystemTime::now()`.
#[must_use]
pub fn relative_age(created_at: &str, now: SystemTime) -> String {
    let Some(then) = parse_iso(created_at) else {
        return String::new();
    };
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let secs = (now - then).max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 604_800 => format!("{}d", s / 86_400),
        s => format!("{}w", s / 604_800),
    }
}

/// Parse a fixed `YYYY-MM-DDTHH:MM:SSZ` timestamp to a Unix epoch second. `None` on any
/// deviation, so a malformed value yields an empty age rather than a wrong one.
// The civil-from-days algorithm reads naturally with the conventional short field names.
#[allow(clippy::many_single_char_names)]
pub(crate) fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, se) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // Days from the civil date (Howard Hinnant's algorithm), then to seconds.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let year_of_era = y - era * 400;
    let day_of_year = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_fails_on_any_failure_else_running_else_success() {
        let snap = |statuses: &[CheckStatus]| PrSnapshot {
            provider: Provider::Github,
            number: 1,
            title: String::new(),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            head_ref: String::new(),
            head_is_fork: false,
            base_ref: String::new(),
            diff_refs: DiffRefs::default(),
            merge: Merge::Clean,
            sync: Sync::InSync,
            checks: statuses.iter().map(|&s| Check { name: "c".into(), status: s }).collect(),
            comments: Vec::new(),
            truncated: false,
            threads_partial: None,
        };
        assert_eq!(snap(&[]).checks_rollup(), None);
        assert_eq!(
            snap(&[CheckStatus::Success, CheckStatus::Success]).checks_rollup(),
            Some(CheckStatus::Success)
        );
        assert_eq!(
            snap(&[CheckStatus::Success, CheckStatus::Running]).checks_rollup(),
            Some(CheckStatus::Running)
        );
        assert_eq!(
            snap(&[CheckStatus::Running, CheckStatus::Failure]).checks_rollup(),
            Some(CheckStatus::Failure)
        );
    }

    #[test]
    fn anchor_locations_name_single_lines_and_ranges() {
        let mut anchor = DiffAnchor {
            path: "src/a.rs".into(),
            old_path: None,
            side: crate::model::Side::New,
            line: 9,
            start_line: None,
            endpoints: None,
        };
        assert_eq!(anchor.location(), "src/a.rs:9");
        anchor.start_line = Some(4);
        assert_eq!(anchor.location(), "src/a.rs:4-9");
        anchor.start_line = Some(9); // a degenerate range reads as one line
        assert_eq!(anchor.location(), "src/a.rs:9");
    }

    #[test]
    fn relative_age_buckets_by_magnitude() {
        // now = 2026-06-27T12:00:00Z
        let now = UNIX_EPOCH
            + std::time::Duration::from_secs(parse_iso("2026-06-27T12:00:00Z").unwrap() as u64);
        assert_eq!(relative_age("2026-06-27T11:55:00Z", now), "5m");
        assert_eq!(relative_age("2026-06-27T10:00:00Z", now), "2h");
        assert_eq!(relative_age("2026-06-24T12:00:00Z", now), "3d");
        assert_eq!(relative_age("2026-06-13T12:00:00Z", now), "2w");
        assert_eq!(relative_age("garbage", now), "");
    }

    #[test]
    fn parse_iso_anchors_the_epoch_and_the_feb_year_branch() {
        // The epoch anchors the civil-from-days math; a Jan/Feb date exercises the `mo <= 2`
        // year-adjust branch that the June fixtures above never hit.
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2000-02-29T00:00:00Z"), Some(951_782_400)); // a leap-day boundary
        assert_eq!(parse_iso("not-a-date"), None);
    }

    #[test]
    fn write_failures_distinguish_rejections_from_unknown_acceptance() {
        let (_, rejected_unknown) =
            sync_failure(CliError::Other("HTTP 422 validation failed".into()), "failed");
        assert!(!rejected_unknown, "a provider rejection is safe to retry explicitly");
        let (unsupported_media, unsupported_unknown) =
            sync_failure(CliError::Other("glab: HTTP 415".into()), "failed");
        assert!(!unsupported_unknown, "HTTP 415 proves GitLab rejected the write");
        assert!(!unsupported_media.contains("press r"), "refresh is not a write retry");

        let (_, disconnected_unknown) = sync_failure(
            CliError::Unreachable { host: "github.com".into(), detail: "timeout".into() },
            "failed",
        );
        assert!(disconnected_unknown, "a lost POST response must never be retried blindly");

        let (_, malformed_response_unknown) =
            sync_failure(CliError::Other("invalid JSON response".into()), "failed");
        assert!(
            malformed_response_unknown,
            "a successful command with an unreadable response may already have posted"
        );
    }

    #[test]
    fn sync_leads_with_unpushed_and_tolerates_a_missing_head() {
        assert_eq!(derive_sync(None), Sync::Unknown);
        assert_eq!(derive_sync(Some((0, 0))), Sync::InSync);
        assert_eq!(derive_sync(Some((2, 0))), Sync::Unpushed(2));
        assert_eq!(derive_sync(Some((0, 3))), Sync::Behind(3));
        assert_eq!(derive_sync(Some((2, 3))), Sync::Unpushed(2)); // diverged → unpushed leads
    }

    #[test]
    fn select_open_takes_the_earliest_candidate_with_any_open_pr() {
        let per = vec![
            vec![],
            vec![(12, "aaa".to_string())],
            vec![(99, "bbb".to_string())], // a later candidate never preempts an earlier one
        ];
        assert_eq!(select_open(&per, Some("zzz")), Pick::One(12));
        assert_eq!(select_open(&[vec![], vec![]], Some("zzz")), Pick::None);
        assert_eq!(select_open(&[], None), Pick::None);
    }

    #[test]
    fn select_open_disambiguates_one_name_by_the_pinned_head_else_surfaces_the_count() {
        let two = vec![vec![(1, "aaa".to_string()), (2, "bbb".to_string())]];
        assert_eq!(select_open(&two, Some("bbb")), Pick::One(2));
        // No pinned HEAD, no exact match, or several exact matches: ambiguous, count shown.
        assert_eq!(select_open(&two, None), Pick::Ambiguous(2));
        assert_eq!(select_open(&two, Some("zzz")), Pick::Ambiguous(2));
        let dup = vec![vec![(1, "aaa".to_string()), (2, "aaa".to_string())]];
        assert_eq!(select_open(&dup, Some("aaa")), Pick::Ambiguous(2));
    }

    #[test]
    fn select_historical_takes_the_newest_created_and_ties_to_the_earlier_candidate() {
        let per = vec![
            vec![(1, "2026-06-01T00:00:00Z".to_string())],
            vec![(2, "2026-06-03T00:00:00Z".to_string())],
            vec![(3, "2026-06-03T00:00:00Z".to_string())], // tie → the earlier candidate keeps
        ];
        assert_eq!(select_historical(&per), Some(2));
        assert_eq!(select_historical(&[vec![], vec![]]), None);
    }

    #[test]
    fn graphql_arguments_always_pin_the_canonical_host() {
        let args = graphql_args(
            "git.example.com",
            "query($o:String!){viewer{login}}",
            &[("o".to_string(), "owner".to_string())],
        );
        assert_eq!(&args[..4], ["api", "graphql", "--hostname", "git.example.com"]);
        assert!(args.windows(2).any(|pair| pair == ["-f", "o=owner"]));
    }

    #[test]
    fn follow_pages_walks_the_cursor_chain_to_its_end() {
        let pages = vec![
            Page {
                nodes: vec![serde_json::json!({"id": "a"})],
                has_next: true,
                cursor: Some("c2".into()),
            },
            Page { nodes: vec![serde_json::json!({"id": "b"})], has_next: false, cursor: None },
        ];
        let mut served = pages.into_iter();
        let mut asked = Vec::new();
        let (nodes, partial) = follow_pages(Some("c1".into()), PAGE_BUDGET, |cursor| {
            asked.push(cursor.to_string());
            Ok(served.next().expect("no page past the end"))
        });
        assert_eq!(asked, ["c1", "c2"]);
        assert_eq!(nodes.len(), 2);
        assert_eq!(partial, None, "a fully walked chain is complete");
    }

    #[test]
    fn follow_pages_without_a_start_cursor_fetches_nothing() {
        let (nodes, partial) =
            follow_pages(None, PAGE_BUDGET, |_| -> Result<Page, CliError> { panic!("no fetch") });
        assert!(nodes.is_empty());
        assert_eq!(partial, None);
    }

    #[test]
    fn follow_pages_keeps_the_prefix_and_marks_the_failure_when_a_page_breaks() {
        let mut calls = 0;
        let (nodes, partial) = follow_pages(Some("c1".into()), PAGE_BUDGET, |_| {
            calls += 1;
            if calls == 1 {
                Ok(Page {
                    nodes: vec![serde_json::json!({"id": "a"})],
                    has_next: true,
                    cursor: Some("c2".into()),
                })
            } else {
                Err(CliError::Other("HTTP 502".into()))
            }
        });
        assert_eq!(nodes.len(), 1, "what arrived before the failure is kept");
        match partial {
            Some(PartialReason::PageFailed(message)) => assert!(message.contains("HTTP 502")),
            other => panic!("expected PageFailed, got {other:?}"),
        }
    }

    #[test]
    fn follow_pages_stops_with_an_explicit_cap_when_the_budget_runs_out() {
        // An empty page that always reports more: the walk must terminate via the budget,
        // never silently pretend completeness.
        let (nodes, partial) = follow_pages(Some("c".into()), 3, |_| {
            Ok(Page { nodes: Vec::new(), has_next: true, cursor: Some("c".into()) })
        });
        assert!(nodes.is_empty(), "empty pages are tolerated");
        assert_eq!(partial, Some(PartialReason::Capped));

        // A claimed next page without a cursor cannot loop.
        let (_, partial) = follow_pages(Some("c".into()), 3, |_| {
            Ok(Page { nodes: Vec::new(), has_next: true, cursor: None })
        });
        assert_eq!(partial, Some(PartialReason::Capped));
    }

    #[test]
    fn append_nodes_drops_rows_the_first_page_already_holds() {
        let mut conn = serde_json::json!({"nodes": [{"id": "a"}, {"id": "b"}]});
        append_nodes(
            &mut conn,
            vec![
                serde_json::json!({"id": "b"}), // repeated when the list shifted between pages
                serde_json::json!({"id": "c"}),
                serde_json::json!({"noid": true}), // unkeyed rows are kept, not guessed about
            ],
        );
        let ids: Vec<_> =
            conn["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str()).collect();
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[2], Some("c"));
        assert_eq!(ids[3], None);
    }

    #[test]
    fn merged_reply_pages_read_in_chronological_thread_order() {
        let reply = |id: &str, at: &str| RemoteReply {
            id: id.into(),
            author: "a".into(),
            body: String::new(),
            created_at: at.into(),
        };
        // A provider may return follow-up pages out of order relative to the first page.
        let mut replies = vec![
            reply("2", "2026-06-02T00:00:00Z"),
            reply("3", "2026-06-03T00:00:00Z"),
            reply("1", "2026-06-01T00:00:00Z"),
        ];
        sort_replies(&mut replies);
        let order: Vec<_> = replies.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(order, ["1", "2", "3"]);
    }

    #[test]
    fn remedies_name_the_provider_tool() {
        assert!(PrView::NoCli("glab").retry_remedy().unwrap().contains("install `glab`"));
        let remedy = PrView::NotAuthed { tool: "glab", host: "gitlab.example.com".into() }
            .retry_remedy()
            .unwrap();
        assert!(remedy.contains("glab auth login --hostname gitlab.example.com"));
        let wall = PrView::ApiUnreachable {
            host: "gitlab.selfhosted.example.com".into(),
            detail: "HTTP 403".into(),
        };
        assert!(wall.retry_remedy().unwrap().contains("check VPN/proxy"));
        assert_eq!(PrView::NeedsSupportedOrigin.retry_remedy(), None);
    }
}
