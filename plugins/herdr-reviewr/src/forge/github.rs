//! The GitHub provider: read one pull request through explicitly hosted `gh` GraphQL calls.
//!
//! See `specs/forge-host.md`. Everything here is GitHub-shaped — the `gh` runner, the GraphQL
//! dialect, and the normalisation from GitHub's vocabulary into the provider-neutral model in
//! [`super`]. The dispatch in [`super::fetch_cancellable`] owns origin classification; this
//! module only ever sees a target already known to be a GitHub repository.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::diff::{PatchFile, PatchSet};
use crate::model::{ChangeKind, ChangedFile};

use super::{
    Check, CheckStatus, CliError, Comment, CommentKind, Merge, Pick, PrFetchInput, PrListItem,
    PrSnapshot, PrState, PrView, Provider, Sync, derive_sync, select_historical, select_open,
};

/// The CLI this provider shells out to; names the tool in degraded states and remedies.
const TOOL: &str = "gh";

/// Read GitHub for one already-dispatched repository target.
pub(crate) fn fetch(
    repo: &Path,
    target: &crate::git::RepoTarget,
    input: &PrFetchInput,
    cancelled: &AtomicBool,
) -> PrView {
    match fetch_inner(repo, target, input, cancelled) {
        Ok(view) => view,
        Err(error) => error.into(),
    }
}

fn fetch_inner(
    repo: &Path,
    target: &crate::git::RepoTarget,
    input: &PrFetchInput,
    cancelled: &AtomicBool,
) -> Result<PrView, CliError> {
    let target = FetchTarget {
        repo,
        host: &target.host,
        owner: &target.owner,
        name: &target.name,
        cancelled,
    };
    // A picker-pinned number skips resolution and reads that PR directly. Otherwise resolve
    // the open PR across all candidates in one aliased call, then read its detail directly —
    // `mergeable` only populates on direct access, never through the list connection
    // (`specs/forge-host.md`).
    let number =
        if let Some(pinned) = input.pinned { pinned } else { resolve_number(&target, input)? };
    let mut detail = pr_detail(&target, number)?;
    let node = &mut detail["data"]["repository"]["pullRequest"];
    if node.is_null() {
        return Ok(PrView::NoPr(input.candidates.clone()));
    }
    let completion = complete_comment_surfaces(&target, number, node);
    // Sync compares the fetch's pinned HEAD to the PR head, so a checkout or commit landing
    // mid-fetch never pairs one branch's PR with another branch's count.
    let pr_head = node["headRefOid"].as_str().unwrap_or_default();
    let sync = match input.head_oid.as_deref() {
        Some(pin) if !pr_head.is_empty() => derive_sync(
            crate::git::ahead_behind_oids(repo, pin, pr_head).map_err(|e| CliError::Other(e.0))?,
        ),
        _ => Sync::Unknown,
    };
    Ok(PrView::Pr(Box::new(build_snapshot(node, sync, completion))))
}

/// The outcome of walking the comment surfaces' cursors: why the thread list is a prefix
/// (if it is) and, per thread node id, why that thread's replies are.
#[derive(Default)]
struct SurfaceCompletion {
    threads_partial: Option<super::PartialReason>,
    thread_partials: std::collections::HashMap<String, super::PartialReason>,
}

/// Walk the prose-comment, thread-list, and per-thread reply cursors to their end, appending
/// rows into `pr`'s connections in place. A fully walked connection gets `hasNextPage:false`
/// so downstream reads see honest flags; a failed or budget-capped walk leaves the flag and
/// records why. Failures here never fail the fetch — the snapshot shows an explicit prefix.
fn complete_comment_surfaces(
    target: &FetchTarget<'_>,
    number: u64,
    pr: &mut Value,
) -> SurfaceCompletion {
    let mut completion = SurfaceCompletion::default();

    // Plain conversation comments.
    if let Some(partial) = complete_connection(&mut pr["comments"], |cursor| {
        let q = format!(
            "query($o:String!,$n:String!,$c:String!){{repository(owner:$o,name:$n){{\
             pullRequest(number:{number}){{comments(first:100, after:$c){{\
             pageInfo{{hasNextPage endCursor}} nodes{{{PROSE_NODE}}}}}}}}}}}"
        );
        let v = run_page_query(target, &q, cursor)?;
        Ok(page_of(&v["data"]["repository"]["pullRequest"]["comments"]))
    }) {
        // No per-row home for a prose prefix: the surviving `hasNextPage` flag feeds the
        // snapshot's truncation marker, and the reason folds into the thread-list state.
        completion.threads_partial = Some(partial);
    }

    // The thread list itself.
    if let Some(partial) = complete_connection(&mut pr["reviewThreads"], |cursor| {
        let q = format!(
            "query($o:String!,$n:String!,$c:String!){{repository(owner:$o,name:$n){{\
             pullRequest(number:{number}){{reviewThreads(first:100, after:$c){{\
             pageInfo{{hasNextPage endCursor}} nodes{{{THREAD_NODE}}}}}}}}}}}"
        );
        let v = run_page_query(target, &q, cursor)?;
        Ok(page_of(&v["data"]["repository"]["pullRequest"]["reviewThreads"]))
    }) {
        completion.threads_partial = Some(partial);
    }

    // Each thread's replies, addressed directly by node id so one giant thread cannot force
    // re-walking the list.
    let thread_ids: Vec<String> = pr["reviewThreads"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["comments"]["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false))
        .filter_map(|t| t["id"].as_str().map(str::to_owned))
        .collect();
    for thread_id in thread_ids {
        let Some(thread) = pr["reviewThreads"]["nodes"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .find(|t| t["id"].as_str() == Some(thread_id.as_str()))
        else {
            continue;
        };
        if let Some(partial) = complete_connection(&mut thread["comments"], |cursor| {
            let q = "query($id:ID!,$c:String!){node(id:$id){... on PullRequestReviewThread{\
                 comments(first:100, after:$c){pageInfo{hasNextPage endCursor} \
                 nodes{id databaseId author{login} body createdAt diffHunk}}}}}";
            let vars =
                [("id".to_string(), thread_id.clone()), ("c".to_string(), cursor.to_string())];
            let v = graphql(target.repo, target.host, q, &vars, target.cancelled)?;
            Ok(page_of(&v["data"]["node"]["comments"]))
        }) {
            completion.thread_partials.insert(thread_id, partial);
        }
    }
    completion
}

/// Walk one connection's remaining pages into it. On a complete walk the connection's
/// `hasNextPage` drops to `false`; otherwise the flag survives and the reason is returned.
fn complete_connection(
    connection: &mut Value,
    fetch: impl FnMut(&str) -> Result<super::Page, CliError>,
) -> Option<super::PartialReason> {
    if !connection["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
        return None;
    }
    let start = connection["pageInfo"]["endCursor"].as_str().map(str::to_owned);
    if start.is_none() {
        // More rows exist but the page carried no cursor to reach them.
        return Some(super::PartialReason::Capped);
    }
    let (nodes, partial) = super::follow_pages(start, super::PAGE_BUDGET, fetch);
    super::append_nodes(connection, nodes);
    if partial.is_none() {
        connection["pageInfo"]["hasNextPage"] = Value::Bool(false);
    }
    partial
}

/// One fetched page from a connection's JSON.
fn page_of(connection: &Value) -> super::Page {
    super::Page {
        nodes: connection["nodes"].as_array().cloned().unwrap_or_default(),
        has_next: connection["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false),
        cursor: connection["pageInfo"]["endCursor"].as_str().map(str::to_owned),
    }
}

/// Run one repository-scoped page query with the standard owner/name variables plus the
/// page cursor, passed as a variable like every other caller-supplied string.
fn run_page_query(target: &FetchTarget<'_>, query: &str, cursor: &str) -> Result<Value, CliError> {
    let vars = [
        ("o".to_string(), target.owner.to_string()),
        ("n".to_string(), target.name.to_string()),
        ("c".to_string(), cursor.to_string()),
    ];
    graphql(target.repo, target.host, query, &vars, target.cancelled)
}

/// The branch-resolved PR number (`specs/forge-host.md` "Resolution").
fn resolve_number(target: &FetchTarget<'_>, input: &PrFetchInput) -> Result<u64, CliError> {
    let open = resolve_candidates(target, &input.candidates, OPEN, "headRefOid")?;
    let number = match select_open(&open, input.head_oid.as_deref()) {
        Pick::One(n) => n,
        Pick::Ambiguous(count) => return Err(CliError::Ambiguous(count)),
        Pick::None => {
            // No open PR anywhere: fall back to the newest-created merged/closed PR.
            let hist = resolve_candidates(target, &input.candidates, HISTORICAL, "createdAt")?;
            match select_historical(&hist) {
                Some(n) => n,
                None => return Err(CliError::NoPr(input.candidates.clone())),
            }
        }
    };
    Ok(number)
}

/// Map a failed `gh`'s stderr to a degraded state by its wording — `gh` has no stable exit
/// codes for these. An unrecognised failure is `Other` → a transient `Error` view.
fn classify_failure(stderr: &str, host: &str) -> CliError {
    let s = stderr.to_lowercase();
    if s.contains("not logged") || s.contains("authentication") || s.contains("gh auth login") {
        CliError::NotAuthed { tool: TOOL, host: host.to_owned() }
    } else if super::unreachable_stderr(&s) {
        // A network wall (VPN/IP allowlist, DNS, proxy down) — the remedy points at the
        // connection, not the forge.
        CliError::Unreachable { host: host.to_owned(), detail: stderr.trim().to_string() }
    } else {
        CliError::Other(stderr.trim().to_string())
    }
}

/// The list filter for the open-PR resolve call. `first:100` keeps the surfaced ambiguity
/// count the real number of open PRs, not a cap.
const OPEN: &str = "states:OPEN, first:100";
/// The list filter for the historical fallback: the newest-created merged/closed PR per name.
const HISTORICAL: &str =
    "states:[MERGED,CLOSED], first:1, orderBy:{field:CREATED_AT, direction:DESC}";

struct FetchTarget<'a> {
    repo: &'a Path,
    host: &'a str,
    owner: &'a str,
    name: &'a str,
    cancelled: &'a AtomicBool,
}

/// The PRs for every candidate name in one aliased GraphQL call — alias `c{i}` ↔ candidate
/// `i`, names passed as variables (never interpolated into the query text). Each returned
/// entry is `(number, extra)` where `extra` is `headRefOid` (open) or `createdAt` (historical).
fn resolve_candidates(
    target: &FetchTarget<'_>,
    candidates: &[String],
    filter: &str,
    extra: &str,
) -> Result<Vec<Vec<(u64, String)>>, CliError> {
    let query = build_resolve_query(candidates.len(), filter, extra);
    let mut vars: Vec<(String, String)> = vec![
        ("o".to_string(), target.owner.to_string()),
        ("n".to_string(), target.name.to_string()),
    ];
    for (i, cand) in candidates.iter().enumerate() {
        vars.push((format!("b{i}"), cand.clone()));
    }
    let v = graphql(target.repo, target.host, &query, &vars, target.cancelled)?;
    Ok(parse_resolve(&v, candidates.len(), extra))
}

/// The fields one review-thread node carries, shared by the detail query and the
/// thread-list pages so a paged thread can never arrive shaped differently.
const THREAD_NODE: &str = "id isResolved isOutdated path line startLine diffSide startDiffSide \
     comments(first:100){totalCount pageInfo{hasNextPage endCursor} \
     nodes{id databaseId author{login} body createdAt diffHunk}}";

/// The fields one plain conversation comment carries, shared with the comment pages.
const PROSE_NODE: &str = "id author{login} body createdAt";

/// All of one PR's state in a single direct GraphQL call — identity, mergeability, checks,
/// reviews, plain comments, and review threads. Each list reads 100 rows per page; the fetch
/// then walks `comments`, `reviewThreads`, and each thread's reply cursors to their end
/// (within [`super::PAGE_BUDGET`]), so completeness is real rather than inferred. `reviews`
/// stays a single `last:100` page — its `hasPreviousPage` only feeds the truncation marker.
fn pr_detail(target: &FetchTarget<'_>, number: u64) -> Result<Value, CliError> {
    let q = format!(
        "query($o:String!,$n:String!){{repository(owner:$o,name:$n){{\
         pullRequest(number:{number}){{\
         number title url isDraft state mergeable mergeStateStatus baseRefName baseRefOid headRefName \
         headRefOid isCrossRepository \
         commits(last:1){{nodes{{commit{{statusCheckRollup{{contexts(first:100){{pageInfo{{hasNextPage}} nodes{{__typename \
         ... on CheckRun{{name status conclusion}} ... on StatusContext{{context state}}}}}}}}}}}}}} \
         reviews(last:100){{pageInfo{{hasPreviousPage}} nodes{{id author{{login}} body state submittedAt}}}} \
         comments(first:100){{pageInfo{{hasNextPage endCursor}} nodes{{{PROSE_NODE}}}}} \
         reviewThreads(first:100){{pageInfo{{hasNextPage endCursor}} nodes{{{THREAD_NODE}}}}}}}}}}}"
    );
    let vars =
        [("o".to_string(), target.owner.to_string()), ("n".to_string(), target.name.to_string())];
    graphql(target.repo, target.host, &q, &vars, target.cancelled)
}

/// Run a GraphQL `query` with `vars` through `gh`, host pinned, stderr classified here.
fn graphql(
    repo: &Path,
    host: &str,
    query: &str,
    vars: &[(String, String)],
    cancelled: &AtomicBool,
) -> Result<Value, CliError> {
    super::graphql(TOOL, classify_failure, repo, host, query, vars, cancelled)
}

/// The repository's PRs for the picker: open ones, then the newest merged/closed — one call,
/// two aliases. `statusCheckRollup.state` is the cheap one-field CI summary per row.
pub(crate) fn list_prs(
    repo: &Path,
    target: &crate::git::RepoTarget,
    cancelled: &AtomicBool,
) -> Result<super::PrListing, CliError> {
    const ROW: &str = "nodes{number title headRefName isDraft state createdAt totalCommentsCount \
                       author{login} commits(last:1){nodes{commit{statusCheckRollup{state}}}}}";
    let q = format!(
        "query($o:String!,$n:String!){{repository(owner:$o,name:$n){{\
         open:pullRequests(states:OPEN, first:100, orderBy:{{field:CREATED_AT, direction:DESC}}){{{ROW}}} \
         done:pullRequests(states:[MERGED,CLOSED], first:50, orderBy:{{field:CREATED_AT, direction:DESC}}){{{ROW}}}}}}}"
    );
    let vars = [("o".to_string(), target.owner.clone()), ("n".to_string(), target.name.clone())];
    let v = graphql(repo, &target.host, &q, &vars, cancelled)?;
    Ok(super::PrListing {
        open: parse_list(&v["data"]["repository"]["open"]["nodes"]),
        done: parse_list(&v["data"]["repository"]["done"]["nodes"]),
    })
}

/// Read the first 100 changed files for an explicitly selected PR. GitHub omits `patch` for
/// binary files and server-side limits; the normalized file keeps that absence explicit.
pub(crate) fn fetch_review_diff(
    repo: &Path,
    target: &crate::git::RepoTarget,
    number: u64,
    cancelled: &AtomicBool,
) -> Result<PatchSet, CliError> {
    let endpoint =
        format!("repos/{}/{}/pulls/{number}/files?per_page=100", target.owner, target.name);
    let args = ["api", "--hostname", target.host.as_str(), endpoint.as_str()];
    let out = match super::run_tool(TOOL, repo, &args, cancelled) {
        Ok(out) => out,
        Err(super::ToolFailure::NoCli(tool)) => return Err(CliError::NoCli(tool)),
        Err(super::ToolFailure::Io(detail)) => return Err(CliError::Other(detail)),
        Err(super::ToolFailure::Stderr(stderr)) => {
            return Err(classify_failure(&stderr, &target.host));
        }
    };
    let value: Value = serde_json::from_str(&out).map_err(|e| CliError::Other(e.to_string()))?;
    parse_review_diff(&value)
}

fn parse_review_diff(value: &Value) -> Result<PatchSet, CliError> {
    let rows = value
        .as_array()
        .ok_or_else(|| CliError::Other("GitHub PR files response was not an array".into()))?;
    let files = rows
        .iter()
        .filter_map(|file| {
            let path = file["filename"].as_str()?.to_string();
            let status = file["status"].as_str().unwrap_or("modified");
            let previous_path = file["previous_filename"].as_str().map(str::to_string);
            Some(PatchFile {
                change: ChangedFile {
                    path,
                    kind: match status {
                        "added" => ChangeKind::Added,
                        "removed" => ChangeKind::Deleted,
                        "renamed" => ChangeKind::Renamed,
                        _ => ChangeKind::Modified,
                    },
                    additions: file["additions"].as_u64().unwrap_or(0) as u32,
                    deletions: file["deletions"].as_u64().unwrap_or(0) as u32,
                    previous_path,
                },
                patch: file["patch"].as_str().map(str::to_string),
                too_large: false,
            })
        })
        .collect();
    Ok(PatchSet { files, truncated: rows.len() == 100 })
}

pub(crate) fn sync_review(
    repo: &Path,
    request: &super::ReviewSyncRequest,
    cancelled: &AtomicBool,
) -> super::ReviewSyncOutcome {
    let mut outcome = super::ReviewSyncOutcome::default();
    let (inline, replies): (Vec<_>, Vec<_>) = request
        .drafts
        .iter()
        .partition(|draft| matches!(draft.action, super::ReviewDraftAction::Inline(_)));

    if !inline.is_empty() {
        let payload = review_payload(request, &inline);
        let endpoint = format!(
            "repos/{}/{}/pulls/{}/reviews",
            request.target.owner, request.target.name, request.number
        );
        match post_json(repo, &request.target.host, &endpoint, &payload, cancelled) {
            Ok(_) => outcome.succeeded.extend(inline.iter().map(|draft| draft.local_id)),
            Err(error) => {
                let ids: Vec<u64> = inline.iter().map(|draft| draft.local_id).collect();
                record_sync_failure(&mut outcome, &ids, error);
            }
        }
    }

    for draft in replies {
        let super::ReviewDraftAction::Reply { remote_id, author } = &draft.action else { continue };
        let (endpoint, body) =
            if let Some(comment_id) = remote_id.as_ref().and_then(|id| id.root_comment_id) {
                (
                    format!(
                        "repos/{}/{}/pulls/{}/comments/{comment_id}/replies",
                        request.target.owner, request.target.name, request.number
                    ),
                    draft.body.clone(),
                )
            } else {
                (
                    format!(
                        "repos/{}/{}/issues/{}/comments",
                        request.target.owner, request.target.name, request.number
                    ),
                    format!("@{author} {}", draft.body),
                )
            };
        let payload = serde_json::json!({"body": body});
        match post_json(repo, &request.target.host, &endpoint, &payload, cancelled) {
            Ok(_) => outcome.succeeded.push(draft.local_id),
            Err(error) => record_sync_failure(&mut outcome, &[draft.local_id], error),
        }
    }
    outcome
}

fn review_payload(request: &super::ReviewSyncRequest, inline: &[&super::ReviewDraft]) -> Value {
    let comments: Vec<Value> = inline
        .iter()
        .filter_map(|draft| {
            let super::ReviewDraftAction::Inline(anchor) = &draft.action else { return None };
            let side = match anchor.side {
                crate::model::Side::New => "RIGHT",
                crate::model::Side::Old => "LEFT",
            };
            let mut value = serde_json::json!({
                "path": anchor.path,
                "body": draft.body,
                "line": anchor.line,
                "side": side,
            });
            if let Some(start) = anchor.start_line {
                value["start_line"] = start.into();
                value["start_side"] = side.into();
            }
            Some(value)
        })
        .collect();
    serde_json::json!({
        "commit_id": request.diff_refs.head_sha,
        "event": "COMMENT",
        "comments": comments,
    })
}

fn post_json(
    repo: &Path,
    host: &str,
    endpoint: &str,
    payload: &Value,
    cancelled: &AtomicBool,
) -> Result<Value, CliError> {
    let body =
        serde_json::to_string(payload).map_err(|error| CliError::Other(error.to_string()))?;
    let args = [
        "api",
        "--hostname",
        host,
        "--method",
        "POST",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
        "--input",
        "-",
        endpoint,
    ];
    let out = match super::run_tool_input(TOOL, repo, &args, Some(&body), cancelled) {
        Ok(out) => out,
        Err(super::ToolFailure::NoCli(tool)) => return Err(CliError::NoCli(tool)),
        Err(super::ToolFailure::Io(detail)) => return Err(CliError::Other(detail)),
        Err(super::ToolFailure::Stderr(stderr)) => return Err(classify_failure(&stderr, host)),
    };
    serde_json::from_str(&out).map_err(|error| CliError::Other(error.to_string()))
}

fn record_sync_failure(outcome: &mut super::ReviewSyncOutcome, ids: &[u64], error: CliError) {
    let (message, uncertain) = super::sync_failure(error, "GitHub review sync failed");
    let target = if uncertain { &mut outcome.uncertain } else { &mut outcome.failed };
    target.extend(ids.iter().map(|id| (*id, message.clone())));
}

// ---- Pure normalization (unit-tested) --------------------------------------------------

/// The picker rows from one list alias's `nodes`. A row without a number is dropped.
fn parse_list(nodes: &Value) -> Vec<PrListItem> {
    nodes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pr| {
            let rollup = pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str();
            Some(PrListItem {
                number: pr["number"].as_u64()?,
                title: pr["title"].as_str().unwrap_or_default().to_string(),
                head_ref: pr["headRefName"].as_str().unwrap_or_default().to_string(),
                author: pr["author"]["login"].as_str().unwrap_or_default().to_string(),
                is_draft: pr["isDraft"].as_bool().unwrap_or(false),
                state: parse_state(pr["state"].as_str().unwrap_or("OPEN")),
                ci: rollup.map(rollup_state),
                created_at: pr["createdAt"].as_str().unwrap_or_default().to_string(),
                comments: pr["totalCommentsCount"].as_u64().unwrap_or(0) as u32,
                threads_open: None,
                threads_resolved: None,
            })
        })
        .collect()
}

/// GitHub's one-word `statusCheckRollup.state` summary as a [`CheckStatus`].
fn rollup_state(state: &str) -> CheckStatus {
    match state {
        "SUCCESS" => CheckStatus::Success,
        "FAILURE" | "ERROR" => CheckStatus::Failure,
        // PENDING / EXPECTED — something is still due.
        _ => CheckStatus::Pending,
    }
}

/// The aliased resolve query for `n` candidates: `c{i}: pullRequests(headRefName:$b{i}, …)`.
/// Branch names ride as `$b{i}` variables, never in the query text.
fn build_resolve_query(n: usize, filter: &str, extra: &str) -> String {
    use std::fmt::Write;
    let mut q = String::from("query($o:String!,$n:String!");
    for i in 0..n {
        let _ = write!(q, ",$b{i}:String!");
    }
    q.push_str("){repository(owner:$o,name:$n){");
    for i in 0..n {
        let _ =
            write!(q, "c{i}:pullRequests(headRefName:$b{i}, {filter}){{nodes{{number {extra}}}}} ");
    }
    q.push_str("}}");
    q
}

/// Per-candidate `(number, extra)` lists from the aliased response, index `i` ↔ alias
/// `c{i}`. A missing or null alias parses as an empty list.
fn parse_resolve(v: &Value, n: usize, extra: &str) -> Vec<Vec<(u64, String)>> {
    (0..n)
        .map(|i| {
            v["data"]["repository"][format!("c{i}").as_str()]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some((p["number"].as_u64()?, p[extra].as_str().unwrap_or_default().to_string()))
                })
                .collect()
        })
        .collect()
}

/// Assemble the snapshot from the PR detail JSON (its connections already page-walked), the
/// computed `sync`, and the walk's outcome.
fn build_snapshot(node: &Value, sync: Sync, completion: SurfaceCompletion) -> PrSnapshot {
    let contexts = &node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
    let rollup = &contexts["nodes"];
    // A surface whose page reports more in the direction it pages is a prefix, not the whole set.
    // Each query asks only for its own flag — `hasNextPage` for the `first:` lists, `hasPreviousPage`
    // for `reviews` (a `last:` list) — so OR-ing both reads whichever applies; the absent one is false.
    let more = |conn: &Value| {
        conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false)
            || conn["pageInfo"]["hasPreviousPage"].as_bool().unwrap_or(false)
    };
    let truncated = more(contexts)
        || more(&node["reviews"])
        || more(&node["comments"])
        || more(&node["reviewThreads"]);
    PrSnapshot {
        provider: Provider::Github,
        number: node["number"].as_u64().unwrap_or_default(),
        title: node["title"].as_str().unwrap_or_default().to_string(),
        url: node["url"].as_str().unwrap_or_default().to_string(),
        state: parse_state(node["state"].as_str().unwrap_or("OPEN")),
        is_draft: node["isDraft"].as_bool().unwrap_or(false),
        head_ref: node["headRefName"].as_str().unwrap_or_default().to_string(),
        head_is_fork: node["isCrossRepository"].as_bool().unwrap_or(false),
        base_ref: node["baseRefName"].as_str().unwrap_or_default().to_string(),
        diff_refs: super::DiffRefs {
            base_sha: node["baseRefOid"].as_str().unwrap_or_default().to_string(),
            start_sha: node["baseRefOid"].as_str().unwrap_or_default().to_string(),
            head_sha: node["headRefOid"].as_str().unwrap_or_default().to_string(),
        },
        merge: derive_merge(node["mergeable"].as_str(), node["mergeStateStatus"].as_str()),
        sync,
        checks: normalize_checks(rollup),
        comments: merge_comments(
            &node["reviews"]["nodes"],
            &node["comments"]["nodes"],
            &node["reviewThreads"]["nodes"],
            &completion.thread_partials,
        ),
        truncated,
        threads_partial: completion.threads_partial,
    }
}

fn parse_state(s: &str) -> PrState {
    match s {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Open,
    }
}

/// Fold GitHub's `mergeable` and `mergeStateStatus` into a [`Merge`]. Only the actionable
/// blockers are surfaced: conflicts and a `blocked` required gate. Everything else — `clean`,
/// `behind`, `unstable`, and still-`unknown` (computing) — folds into `Clean` (shows nothing).
fn derive_merge(mergeable: Option<&str>, state: Option<&str>) -> Merge {
    match (mergeable, state) {
        (Some("CONFLICTING"), _) | (_, Some("DIRTY")) => Merge::Conflicting,
        (_, Some("BLOCKED")) => Merge::Blocked,
        _ => Merge::Clean,
    }
}

/// The latest run per check name, normalised from check runs and commit statuses.
fn normalize_checks(rollup: &Value) -> Vec<Check> {
    let mut out: Vec<Check> = Vec::new();
    for node in rollup.as_array().into_iter().flatten() {
        let name =
            node["name"].as_str().or_else(|| node["context"].as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let status = check_status(node);
        // Latest wins: a later array entry for the same name (a re-run) replaces the earlier.
        if let Some(slot) = out.iter_mut().find(|c| c.name == name) {
            *slot = Check { name, status };
        } else {
            out.push(Check { name, status });
        }
    }
    out
}

/// Normalise one check node — a check run (`status`/`conclusion`) or a commit status (`state`)
/// — to a [`CheckStatus`].
fn check_status(node: &Value) -> CheckStatus {
    // Check runs carry `status`/`conclusion`; commit statuses carry `state`.
    if let Some(state) = node["state"].as_str() {
        return match state {
            "SUCCESS" => CheckStatus::Success,
            "FAILURE" | "ERROR" => CheckStatus::Failure,
            _ => CheckStatus::Pending,
        };
    }
    match node["status"].as_str() {
        Some("COMPLETED") => match node["conclusion"].as_str() {
            Some("SUCCESS") => CheckStatus::Success,
            Some("SKIPPED" | "NEUTRAL") => CheckStatus::Skipped,
            // FAILURE / TIMED_OUT / CANCELLED / ACTION_REQUIRED / a missing conclusion all read
            // as a failed check — something needs attention.
            _ => CheckStatus::Failure,
        },
        Some("IN_PROGRESS") => CheckStatus::Running,
        _ => CheckStatus::Pending,
    }
}

/// Merge the three comment surfaces (GraphQL `reviews`, `comments`, and `reviewThreads` node
/// arrays) into one newest-first list, keeping only a bot's latest PR-level post and each human's.
/// `thread_partials` names the threads whose reply walk failed, so their prefix is marked
/// with the failure rather than a generic cap.
fn merge_comments(
    reviews: &Value,
    issues: &Value,
    threads: &Value,
    thread_partials: &std::collections::HashMap<String, super::PartialReason>,
) -> Vec<Comment> {
    let mut out: Vec<Comment> = Vec::new();

    // Submitted reviews with a non-empty body (the PR-level `review` cards).
    for r in reviews.as_array().into_iter().flatten() {
        let body = r["body"].as_str().unwrap_or("").trim().to_string();
        if body.is_empty() {
            continue;
        }
        out.push(prose_comment(
            CommentKind::Review,
            &r["author"],
            body,
            r["submittedAt"].as_str(),
            r["id"].as_str(),
        ));
    }

    // Plain conversation comments (the `comment` cards).
    for c in issues.as_array().into_iter().flatten() {
        let body = c["body"].as_str().unwrap_or("").trim().to_string();
        if body.is_empty() {
            continue;
        }
        out.push(prose_comment(
            CommentKind::Comment,
            &c["author"],
            body,
            c["createdAt"].as_str(),
            c["id"].as_str(),
        ));
    }

    // Inline review threads (the `finding` cards), with resolved/outdated and replies.
    for t in threads.as_array().into_iter().flatten() {
        let root = &t["comments"]["nodes"][0];
        let login = root["author"]["login"].as_str().unwrap_or("").to_string();
        let path = t["path"].as_str().unwrap_or("");
        let side = match t["diffSide"].as_str() {
            Some("LEFT") => crate::model::Side::Old,
            _ => crate::model::Side::New,
        };
        let diff_anchor = t["line"].as_u64().map(|line| super::DiffAnchor {
            path: path.to_string(),
            old_path: None,
            side,
            line: line as u32,
            start_line: t["startLine"].as_u64().map(|line| line as u32),
            endpoints: None,
        });
        let anchor =
            diff_anchor.as_ref().map_or_else(|| path.to_string(), super::DiffAnchor::location);
        let mut replies: Vec<super::RemoteReply> = t["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(1)
            .map(|reply| super::RemoteReply {
                id: reply["id"].as_str().unwrap_or_default().to_string(),
                author: reply["author"]["login"].as_str().unwrap_or_default().to_string(),
                body: reply["body"].as_str().unwrap_or_default().to_string(),
                created_at: reply["createdAt"].as_str().unwrap_or_default().to_string(),
            })
            .collect();
        super::sort_replies(&mut replies);
        let reply_count =
            t["comments"]["totalCount"].as_u64().unwrap_or(1).saturating_sub(1) as u32;
        // A surviving next-page flag means the walk did not finish: the chain is a prefix,
        // marked with the recorded failure or the page cap. At least one reply is missing
        // even when a stale totalCount claims otherwise.
        let thread_id = t["id"].as_str().unwrap_or_default().to_string();
        let replies_state = if t["comments"]["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
            super::RepliesState::Partial {
                missing: (reply_count.saturating_sub(replies.len() as u32)).max(1),
                reason: thread_partials
                    .get(&thread_id)
                    .cloned()
                    .unwrap_or(super::PartialReason::Capped),
            }
        } else {
            super::RepliesState::Complete
        };
        out.push(Comment {
            kind: CommentKind::Finding,
            author_is_bot: is_bot(&login),
            author: login,
            anchor,
            body: root["body"].as_str().unwrap_or("").trim().to_string(),
            snippet: root["diffHunk"].as_str().filter(|h| !h.is_empty()).map(str::to_string),
            created_at: root["createdAt"].as_str().unwrap_or("").to_string(),
            is_resolved: t["isResolved"].as_bool().unwrap_or(false),
            is_outdated: t["isOutdated"].as_bool().unwrap_or(false),
            reply_count,
            replies,
            replies_state,
            diff_anchor,
            remote_id: Some(super::RemoteCommentId {
                thread_id,
                root_comment_id: root["databaseId"].as_u64(),
            }),
        });
    }

    super::dedup_bot_prose(&mut out);
    // Newest first: ISO-8601 `…Z` strings sort lexically in chronological order.
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

fn prose_comment(
    kind: CommentKind,
    user: &Value,
    body: String,
    created_at: Option<&str>,
    node_id: Option<&str>,
) -> Comment {
    let login = user["login"].as_str().unwrap_or("").to_string();
    let anchor = match kind {
        CommentKind::Review => "review",
        _ => "comment",
    };
    Comment {
        kind,
        author_is_bot: is_bot(&login),
        author: login,
        anchor: anchor.to_string(),
        body,
        snippet: None,
        created_at: created_at.unwrap_or("").to_string(),
        is_resolved: false,
        is_outdated: false,
        reply_count: 0,
        replies: Vec::new(),
        replies_state: super::RepliesState::Complete,
        diff_anchor: None,
        remote_id: node_id
            .map(|id| super::RemoteCommentId { thread_id: id.to_string(), root_comment_id: None }),
    }
}

/// Whether a GitHub login is an app/bot (`…[bot]`).
fn is_bot(login: &str) -> bool {
    login.ends_with("[bot]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_surfaces_only_conflicts_and_blocked() {
        assert_eq!(derive_merge(Some("CONFLICTING"), Some("DIRTY")), Merge::Conflicting);
        assert_eq!(derive_merge(Some("MERGEABLE"), Some("BLOCKED")), Merge::Blocked);
        // Everything non-actionable folds into Clean: clean, behind, unstable, still-computing.
        assert_eq!(derive_merge(Some("MERGEABLE"), Some("CLEAN")), Merge::Clean);
        assert_eq!(derive_merge(Some("MERGEABLE"), Some("BEHIND")), Merge::Clean);
        assert_eq!(derive_merge(Some("MERGEABLE"), Some("UNSTABLE")), Merge::Clean);
        assert_eq!(derive_merge(Some("UNKNOWN"), Some("UNKNOWN")), Merge::Clean);
        // DIRTY means conflicts even while mergeability is still UNKNOWN or the field is missing.
        assert_eq!(derive_merge(Some("UNKNOWN"), Some("DIRTY")), Merge::Conflicting);
        assert_eq!(derive_merge(None, Some("DIRTY")), Merge::Conflicting);
        assert_eq!(derive_merge(None, None), Merge::Clean);
    }

    #[test]
    fn parse_state_maps_the_three_github_lifecycles() {
        assert_eq!(parse_state("MERGED"), PrState::Merged);
        assert_eq!(parse_state("CLOSED"), PrState::Closed);
        assert_eq!(parse_state("OPEN"), PrState::Open);
        assert_eq!(parse_state("anything-else"), PrState::Open); // default is the live case
    }

    #[test]
    fn inline_drafts_form_one_grouped_review_payload() {
        let request = super::super::ReviewSyncRequest::new(
            crate::git::RepoTarget {
                provider: Provider::Github,
                host: "github.com".into(),
                owner: "owner".into(),
                name: "repo".into(),
            },
            7,
            super::super::DiffRefs {
                base_sha: String::new(),
                start_sha: String::new(),
                head_sha: "head".into(),
            },
            vec![
                super::super::ReviewDraft {
                    local_id: 1,
                    action: super::super::ReviewDraftAction::Inline(super::super::DiffAnchor {
                        path: "a.rs".into(),
                        old_path: None,
                        side: crate::model::Side::New,
                        line: 12,
                        start_line: None,
                        endpoints: None,
                    }),
                    body: "first".into(),
                },
                super::super::ReviewDraft {
                    local_id: 2,
                    action: super::super::ReviewDraftAction::Inline(super::super::DiffAnchor {
                        path: "b.rs".into(),
                        old_path: None,
                        side: crate::model::Side::Old,
                        line: 8,
                        start_line: Some(6),
                        endpoints: None,
                    }),
                    body: "second".into(),
                },
            ],
        );
        let drafts: Vec<_> = request.drafts.iter().collect();
        let payload = review_payload(&request, &drafts);
        assert_eq!(payload["event"], "COMMENT");
        assert_eq!(payload["commit_id"], "head");
        assert_eq!(payload["comments"].as_array().unwrap().len(), 2);
        assert_eq!(payload["comments"][1]["side"], "LEFT");
        assert_eq!(payload["comments"][1]["start_line"], 6);
    }

    #[test]
    fn pr_files_normalize_renames_stats_and_missing_patches() {
        let value = serde_json::json!([
            {
                "filename": "src/new.rs", "previous_filename": "src/old.rs",
                "status": "renamed", "additions": 3, "deletions": 1,
                "patch": "@@ -1 +1 @@\n-old\n+new"
            },
            {
                "filename": "asset.bin", "status": "modified",
                "additions": 0, "deletions": 0
            }
        ]);
        let patch = parse_review_diff(&value).unwrap();
        assert_eq!(patch.files.len(), 2);
        assert_eq!(patch.files[0].change.kind, ChangeKind::Renamed);
        assert_eq!(patch.files[0].change.previous_path.as_deref(), Some("src/old.rs"));
        assert_eq!((patch.files[0].change.additions, patch.files[0].change.deletions), (3, 1));
        assert!(patch.files[1].patch.is_none());
        assert!(!patch.truncated);
    }

    #[test]
    fn truncated_flips_when_any_capped_surface_has_a_next_page() {
        let base = serde_json::json!({
            "number": 1, "title": "t", "url": "u", "state": "OPEN", "isDraft": false,
            "baseRefName": "main", "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
            "commits": {"nodes": [{"commit": {"statusCheckRollup":
                {"contexts": {"pageInfo": {"hasNextPage": false}, "nodes": []}}}}]},
            "reviews": {"pageInfo": {"hasNextPage": false}, "nodes": []},
            "comments": {"pageInfo": {"hasNextPage": false}, "nodes": []},
            "reviewThreads": {"pageInfo": {"hasNextPage": false}, "nodes": []}
        });
        assert!(
            !build_snapshot(&base, Sync::InSync, SurfaceCompletion::default()).truncated,
            "all pages complete → not truncated"
        );

        let mut comments_more = base.clone();
        comments_more["comments"]["pageInfo"]["hasNextPage"] = serde_json::json!(true);
        assert!(
            build_snapshot(&comments_more, Sync::InSync, SurfaceCompletion::default()).truncated
        );

        let mut threads_more = base.clone();
        threads_more["reviewThreads"]["pageInfo"]["hasNextPage"] = serde_json::json!(true);
        assert!(
            build_snapshot(&threads_more, Sync::InSync, SurfaceCompletion::default()).truncated
        );

        let mut checks_more = base.clone();
        checks_more["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["pageInfo"]
            ["hasNextPage"] = serde_json::json!(true);
        assert!(build_snapshot(&checks_more, Sync::InSync, SurfaceCompletion::default()).truncated);

        // `reviews` pages backward (last:100), so its "more exist" flag is `hasPreviousPage` —
        // checking `hasNextPage` here (the old bug) would leave this surface never marked.
        let mut reviews_more = base.clone();
        reviews_more["reviews"]["pageInfo"]["hasPreviousPage"] = serde_json::json!(true);
        assert!(
            build_snapshot(&reviews_more, Sync::InSync, SurfaceCompletion::default()).truncated
        );
    }

    #[test]
    fn checks_take_the_latest_run_per_name() {
        let rollup = serde_json::json!([
            {"__typename": "CheckRun", "name": "tests", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"__typename": "CheckRun", "name": "tests", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS"},
            {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SKIPPED"},
            {"__typename": "CheckRun", "name": "codeql", "status": "COMPLETED", "conclusion": "NEUTRAL"},
            {"__typename": "StatusContext", "context": "deploy", "state": "PENDING"}
        ]);
        let checks = normalize_checks(&rollup);
        assert_eq!(checks.len(), 5);
        let tests = checks.iter().find(|c| c.name == "tests").unwrap();
        assert_eq!(tests.status, CheckStatus::Success); // the re-run won
        assert_eq!(checks.iter().find(|c| c.name == "build").unwrap().status, CheckStatus::Running);
        // SKIPPED and NEUTRAL both fold to Skipped — neither fails nor blocks the rollup.
        assert_eq!(checks.iter().find(|c| c.name == "lint").unwrap().status, CheckStatus::Skipped);
        assert_eq!(
            checks.iter().find(|c| c.name == "codeql").unwrap().status,
            CheckStatus::Skipped
        );
        assert_eq!(
            checks.iter().find(|c| c.name == "deploy").unwrap().status,
            CheckStatus::Pending
        );
    }

    #[test]
    fn resolve_query_aliases_candidates_and_never_inlines_names() {
        let q = build_resolve_query(2, OPEN, "headRefOid");
        assert_eq!(
            q,
            "query($o:String!,$n:String!,$b0:String!,$b1:String!)\
             {repository(owner:$o,name:$n){\
             c0:pullRequests(headRefName:$b0, states:OPEN, first:100){nodes{number headRefOid}} \
             c1:pullRequests(headRefName:$b1, states:OPEN, first:100){nodes{number headRefOid}} }}"
        );
        let h = build_resolve_query(1, HISTORICAL, "createdAt");
        assert!(h.contains(
            "states:[MERGED,CLOSED], first:1, orderBy:{field:CREATED_AT, direction:DESC}"
        ));
        assert!(h.contains("nodes{number createdAt}"));
    }

    #[test]
    fn parse_resolve_maps_aliases_in_order_and_null_to_empty() {
        let v = serde_json::json!({"data": {"repository": {
            "c0": {"nodes": [{"number": 7, "headRefOid": "abc"}]},
            "c1": null,
            "c2": {"nodes": [{"number": 9, "headRefOid": "def"}, {"number": 10, "headRefOid": "ghi"}]}
        }}});
        let per = parse_resolve(&v, 3, "headRefOid");
        assert_eq!(per[0], [(7, "abc".to_string())]);
        assert!(per[1].is_empty());
        assert_eq!(per[2], [(9, "def".to_string()), (10, "ghi".to_string())]);
    }

    #[test]
    fn snapshot_carries_the_head_ref_fork_marker_and_provider() {
        let node = serde_json::json!({
            "number": 5, "title": "t", "url": "u", "state": "OPEN", "isDraft": false,
            "headRefName": "persiyanov/feature", "isCrossRepository": true, "baseRefName": "main",
            "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
            "commits": {"nodes": []}, "reviews": {"nodes": []},
            "comments": {"nodes": []}, "reviewThreads": {"nodes": []}
        });
        let s = build_snapshot(&node, Sync::InSync, SurfaceCompletion::default());
        assert_eq!(s.head_ref, "persiyanov/feature");
        assert!(s.head_is_fork);
        assert_eq!(s.provider, Provider::Github);
        // Absent fields default rather than fail — a mid-rollout API response degrades soft.
        let bare = serde_json::json!({"number": 5});
        let s = build_snapshot(&bare, Sync::InSync, SurfaceCompletion::default());
        assert_eq!(s.head_ref, "");
        assert!(!s.head_is_fork);
    }

    #[test]
    fn comments_merge_three_surfaces_newest_first() {
        let reviews = serde_json::json!([
            {"author": {"login": "codex[bot]"}, "state": "COMMENTED", "body": "Codex review.", "submittedAt": "2026-06-27T10:00:00Z"}
        ]);
        let issues = serde_json::json!([
            {"author": {"login": "persijano"}, "body": "watch the 404s", "createdAt": "2026-06-27T12:00:00Z"}
        ]);
        let threads = serde_json::json!([
            {"isResolved": false, "isOutdated": true, "path": "a.py", "line": null,
             "comments": {"totalCount": 2, "nodes": [{"author": {"login": "claude[bot]"}, "body": "SSRF", "createdAt": "2026-06-27T11:00:00Z"}]}}
        ]);
        let cs = merge_comments(&reviews, &issues, &threads, &std::collections::HashMap::new());
        assert_eq!(cs.len(), 3);
        // Newest first across all three surfaces — pin the full order so a reversed or
        // unstable comparator fails rather than passing on the endpoints alone.
        assert_eq!(
            cs.iter().map(|c| c.created_at.as_str()).collect::<Vec<_>>(),
            ["2026-06-27T12:00:00Z", "2026-06-27T11:00:00Z", "2026-06-27T10:00:00Z"]
        );
        assert_eq!(cs[0].author, "persijano");
        assert_eq!(cs[0].kind, CommentKind::Comment);
        assert!(!cs[0].author_is_bot);
        assert_eq!(cs[1].kind, CommentKind::Finding);
        assert_eq!(cs[2].kind, CommentKind::Review);
        // The finding carries its thread state, an unanchored line, and one reply.
        let f = cs.iter().find(|c| c.kind == CommentKind::Finding).unwrap();
        assert_eq!(f.anchor, "a.py");
        assert!(f.is_outdated);
        assert_eq!(f.reply_count, 1);
    }

    #[test]
    fn a_completed_thread_reads_complete_and_its_replies_sort_chronologically() {
        // Page merges may append older replies after newer ones; the parsed chain must read
        // in thread order regardless.
        let threads = serde_json::json!([
            {"id": "T1", "isResolved": false, "isOutdated": false, "path": "a.py", "line": 3,
             "comments": {"totalCount": 3, "pageInfo": {"hasNextPage": false},
              "nodes": [
                {"id": "c0", "author": {"login": "root"}, "body": "finding", "createdAt": "2026-06-27T10:00:00Z"},
                {"id": "c2", "author": {"login": "b"}, "body": "second", "createdAt": "2026-06-27T12:00:00Z"},
                {"id": "c1", "author": {"login": "a"}, "body": "first", "createdAt": "2026-06-27T11:00:00Z"}
              ]}}
        ]);
        let cs = merge_comments(
            &serde_json::json!([]),
            &serde_json::json!([]),
            &threads,
            &std::collections::HashMap::new(),
        );
        assert_eq!(cs[0].replies_state, super::super::RepliesState::Complete);
        let order: Vec<_> = cs[0].replies.iter().map(|r| r.body.as_str()).collect();
        assert_eq!(order, ["first", "second"], "replies read in chronological thread order");
    }

    #[test]
    fn an_unfinished_reply_walk_marks_the_thread_partial_never_complete() {
        let threads = serde_json::json!([
            {"id": "T1", "isResolved": false, "isOutdated": false, "path": "a.py", "line": 3,
             "comments": {"totalCount": 12, "pageInfo": {"hasNextPage": true},
              "nodes": [
                {"id": "c0", "author": {"login": "root"}, "body": "finding", "createdAt": "2026-06-27T10:00:00Z"},
                {"id": "c1", "author": {"login": "a"}, "body": "only reply loaded", "createdAt": "2026-06-27T11:00:00Z"}
              ]}}
        ]);
        // Without a recorded failure the surviving flag reads as the page cap…
        let cs = merge_comments(
            &serde_json::json!([]),
            &serde_json::json!([]),
            &threads,
            &std::collections::HashMap::new(),
        );
        assert_eq!(
            cs[0].replies_state,
            super::super::RepliesState::Partial {
                missing: 10,
                reason: super::super::PartialReason::Capped
            }
        );
        // …and a recorded per-thread failure names itself instead.
        let mut partials = std::collections::HashMap::new();
        partials
            .insert("T1".to_string(), super::super::PartialReason::PageFailed("HTTP 502".into()));
        let cs =
            merge_comments(&serde_json::json!([]), &serde_json::json!([]), &threads, &partials);
        match &cs[0].replies_state {
            super::super::RepliesState::Partial {
                missing: 10,
                reason: super::super::PartialReason::PageFailed(message),
            } => assert!(message.contains("HTTP 502")),
            other => panic!("expected a PageFailed partial, got {other:?}"),
        }
    }

    #[test]
    fn a_stale_total_count_still_reads_at_least_one_reply_missing() {
        // totalCount can lag behind the true chain; an unfinished walk must never read as
        // zero missing, which would render as a complete thread.
        let threads = serde_json::json!([
            {"id": "T1", "isResolved": false, "isOutdated": false, "path": "a.py", "line": 3,
             "comments": {"totalCount": 2, "pageInfo": {"hasNextPage": true},
              "nodes": [
                {"id": "c0", "author": {"login": "root"}, "body": "finding", "createdAt": "2026-06-27T10:00:00Z"},
                {"id": "c1", "author": {"login": "a"}, "body": "reply", "createdAt": "2026-06-27T11:00:00Z"}
              ]}}
        ]);
        let cs = merge_comments(
            &serde_json::json!([]),
            &serde_json::json!([]),
            &threads,
            &std::collections::HashMap::new(),
        );
        assert_eq!(
            cs[0].replies_state,
            super::super::RepliesState::Partial {
                missing: 1,
                reason: super::super::PartialReason::Capped
            }
        );
    }

    #[test]
    fn a_multi_line_thread_reads_back_with_its_full_range() {
        let threads = serde_json::json!([
            {"isResolved": false, "isOutdated": false, "path": "b.py",
             "line": 9, "startLine": 4, "diffSide": "RIGHT",
             "comments": {"totalCount": 1, "nodes": [{"author": {"login": "persijano"},
              "body": "this whole block", "createdAt": "2026-06-27T11:00:00Z"}]}}
        ]);
        let cs = merge_comments(
            &serde_json::json!([]),
            &serde_json::json!([]),
            &threads,
            &std::collections::HashMap::new(),
        );
        assert_eq!(cs[0].anchor, "b.py:4-9", "the anchor names the whole range");
        let anchor = cs[0].diff_anchor.as_ref().unwrap();
        assert_eq!((anchor.start_line, anchor.line), (Some(4), 9));
    }

    #[test]
    fn a_bots_prose_collapses_to_its_latest_a_humans_is_kept() {
        let reviews = serde_json::json!([
            {"author": {"login": "claude[bot]"}, "body": "old review", "submittedAt": "2026-06-27T09:00:00Z"},
            {"author": {"login": "claude[bot]"}, "body": "new review", "submittedAt": "2026-06-27T10:00:00Z"},
            {"author": {"login": "persijano"}, "body": "note one", "submittedAt": "2026-06-27T09:30:00Z"},
            {"author": {"login": "persijano"}, "body": "note two", "submittedAt": "2026-06-27T09:45:00Z"}
        ]);
        let cs = merge_comments(
            &reviews,
            &serde_json::json!([]),
            &serde_json::json!([]),
            &std::collections::HashMap::new(),
        );
        let claude: Vec<_> = cs.iter().filter(|c| c.author == "claude[bot]").collect();
        assert_eq!(claude.len(), 1); // only the latest bot review
        assert_eq!(claude[0].body, "new review");
        assert_eq!(cs.iter().filter(|c| c.author == "persijano").count(), 2); // both human notes
    }

    #[test]
    fn a_bots_findings_are_each_kept_even_as_its_prose_collapses() {
        // Inline findings anchor to distinct lines, so — unlike a bot's PR-level prose — they
        // are never collapsed: two findings from the same bot both survive, the prose folds to one.
        let reviews = serde_json::json!([
            {"author": {"login": "claude[bot]"}, "body": "old prose", "submittedAt": "2026-06-27T09:00:00Z"},
            {"author": {"login": "claude[bot]"}, "body": "new prose", "submittedAt": "2026-06-27T09:30:00Z"}
        ]);
        let threads = serde_json::json!([
            {"isResolved": false, "isOutdated": false, "path": "a.py", "line": 10,
             "comments": {"totalCount": 1, "nodes": [{"author": {"login": "claude[bot]"}, "body": "finding one", "createdAt": "2026-06-27T10:00:00Z"}]}},
            {"isResolved": false, "isOutdated": false, "path": "b.py", "line": 20,
             "comments": {"totalCount": 1, "nodes": [{"author": {"login": "claude[bot]"}, "body": "finding two", "createdAt": "2026-06-27T11:00:00Z"}]}}
        ]);
        let cs = merge_comments(
            &reviews,
            &serde_json::json!([]),
            &threads,
            &std::collections::HashMap::new(),
        );
        assert_eq!(cs.iter().filter(|c| c.kind == CommentKind::Finding).count(), 2);
        assert_eq!(cs.iter().filter(|c| c.kind == CommentKind::Review).count(), 1); // prose collapsed
    }

    #[test]
    fn pr_list_rows_parse_state_and_ci_and_drop_numberless_ones() {
        let nodes = serde_json::json!([
            {"number": 8, "title": "Add thing", "headRefName": "feat/x", "isDraft": true,
             "state": "OPEN", "createdAt": "2026-07-02T00:00:00Z", "totalCommentsCount": 4,
             "author": {"login": "yassimba"},
             "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]}},
            {"title": "ghost row"},
            {"number": 3, "title": "Fix other", "headRefName": "fix/y", "isDraft": false,
             "state": "MERGED", "createdAt": "2026-06-01T00:00:00Z", "author": null,
             "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]}}
        ]);
        let items = parse_list(&nodes);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].number, 8);
        assert!(items[0].is_draft);
        assert_eq!(items[0].ci, Some(CheckStatus::Success));
        assert_eq!(items[0].comments, 4);
        assert_eq!(items[0].threads_open, None, "GitHub has no cheap thread verdict");
        assert_eq!(items[1].state, PrState::Merged);
        assert_eq!(items[1].ci, Some(CheckStatus::Failure));
        assert_eq!(items[1].author, ""); // a deleted author degrades soft
        // A PR with no checks at all has no CI verdict, not a fake pending one.
        let bare = serde_json::json!([{"number": 1, "state": "CLOSED",
            "commits": {"nodes": [{"commit": {"statusCheckRollup": null}}]}}]);
        assert_eq!(parse_list(&bare)[0].ci, None);
    }

    #[test]
    fn gh_failure_classifies_by_stderr_wording() {
        assert_eq!(
            classify_failure("gh auth login required", "github.example.com"),
            CliError::NotAuthed { tool: "gh", host: "github.example.com".to_string() }
        );
        assert_eq!(
            classify_failure("You are not logged into any GitHub hosts", "github.com"),
            CliError::NotAuthed { tool: "gh", host: "github.com".to_string() }
        );
        assert_eq!(
            classify_failure("HTTP 500 something", "github.com"),
            CliError::Other("HTTP 500 something".into())
        );
        // A network wall (VPN/IP allowlist, DNS) is unreachable, not a generic error — the
        // remedy points at the connection, not the forge.
        assert_eq!(
            classify_failure("dial tcp: connection refused", "github.example.com"),
            CliError::Unreachable {
                host: "github.example.com".to_string(),
                detail: "dial tcp: connection refused".to_string()
            }
        );
    }
}
