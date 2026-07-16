//! The GitLab provider: read one merge request through explicitly hosted `glab` GraphQL calls.
//!
//! Mirrors [`super::github`] shape-for-shape: a two-phase fetch (resolve the MR across the
//! candidate branches, then read its detail directly), pure fixture-tested normalisation from
//! GitLab's vocabulary into the provider-neutral model, and stderr-classified degradation.
//! `glab` resolves via `PATH` — wrapper shims (proxy setups) stay in charge — and every call
//! pins `--hostname` to the canonical origin host so ambient `GITLAB_HOST` cannot redirect it.

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
const TOOL: &str = "glab";

/// Read GitLab for one already-dispatched repository target.
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
    let target = FetchTarget { repo, host: &target.host, full_path: target.full_path(), cancelled };
    // A picker-pinned iid skips resolution and reads that MR directly. Otherwise resolve the
    // open MR across all candidates in one aliased call, then read its detail directly (same
    // two-phase shape as GitHub; keeps the resolve payload tiny).
    let iid = if let Some(pinned) = input.pinned { pinned } else { resolve_iid(&target, input)? };
    let mut detail = mr_detail(&target, iid)?;
    let node = &mut detail["data"]["project"]["mergeRequest"];
    if node.is_null() {
        return Ok(PrView::NoPr(input.candidates.clone()));
    }
    let completion = complete_discussions(&target, iid, node);
    // Sync compares the fetch's pinned HEAD to the MR head, so a checkout or commit landing
    // mid-fetch never pairs one branch's MR with another branch's count.
    let mr_head = node["diffHeadSha"].as_str().unwrap_or_default();
    let sync = match input.head_oid.as_deref() {
        Some(pin) if !mr_head.is_empty() => derive_sync(
            crate::git::ahead_behind_oids(repo, pin, mr_head).map_err(|e| CliError::Other(e.0))?,
        ),
        _ => Sync::Unknown,
    };
    Ok(PrView::Pr(Box::new(build_snapshot(node, sync, completion))))
}

/// The outcome of completing the discussion surface: why the discussion list is a prefix
/// (if it is) and, per discussion id, why that discussion's notes are.
#[derive(Default)]
struct DiscussionCompletion {
    threads_partial: Option<super::PartialReason>,
    note_partials: std::collections::HashMap<String, super::PartialReason>,
}

/// Walk the discussion list's cursors to their end, then complete each discussion whose
/// note list is capped through the REST single-discussion endpoint (which returns the whole
/// note set). Appended rows land in `mr`'s JSON in place; a fully walked connection gets
/// `hasNextPage:false`, a failed one keeps its flag and records why. Failures here never
/// fail the fetch — the snapshot shows an explicit prefix.
fn complete_discussions(
    target: &FetchTarget<'_>,
    iid: u64,
    mr: &mut Value,
) -> DiscussionCompletion {
    let mut completion = DiscussionCompletion::default();

    // The discussion list itself.
    let discussions = &mut mr["discussions"];
    if discussions["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
        let start = discussions["pageInfo"]["endCursor"].as_str().map(str::to_owned);
        let (nodes, partial) = if start.is_none() {
            (Vec::new(), Some(super::PartialReason::Capped))
        } else {
            super::follow_pages(start, super::PAGE_BUDGET, |cursor| {
                let q = format!(
                    "query($p:ID!,$c:String!){{project(fullPath:$p){{\
                     mergeRequest(iid:\"{iid}\"){{discussions(first:100, after:$c){{\
                     pageInfo{{hasNextPage endCursor}} nodes{{{DISCUSSION_NODE}}}}}}}}}}}"
                );
                let vars = [
                    ("p".to_string(), target.full_path.clone()),
                    ("c".to_string(), cursor.to_string()),
                ];
                let v = graphql(target, &q, &vars)?;
                let conn = &v["data"]["project"]["mergeRequest"]["discussions"];
                Ok(super::Page {
                    nodes: conn["nodes"].as_array().cloned().unwrap_or_default(),
                    has_next: conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false),
                    cursor: conn["pageInfo"]["endCursor"].as_str().map(str::to_owned),
                })
            })
        };
        super::append_nodes(discussions, nodes);
        if partial.is_none() {
            discussions["pageInfo"]["hasNextPage"] = Value::Bool(false);
        }
        completion.threads_partial = partial;
    }

    // Each discussion whose first notes page was capped. GitLab's GraphQL cannot address one
    // discussion, but the REST single-discussion endpoint returns its complete note list.
    let capped: Vec<String> = mr["discussions"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["notes"]["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false))
        .filter_map(|d| d["id"].as_str().map(str::to_owned))
        .collect();
    for discussion_id in capped {
        let outcome = fetch_full_discussion(target, iid, gid_tail(&discussion_id));
        let Some(discussion) = mr["discussions"]["nodes"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .find(|d| d["id"].as_str() == Some(discussion_id.as_str()))
        else {
            continue;
        };
        match outcome {
            Ok(rest_notes) => {
                let notes = &mut discussion["notes"];
                super::append_nodes_by_key(notes, rest_notes, |node| {
                    node["id"].as_str().map(|id| gid_tail(id).to_string())
                });
                let total = notes["nodes"].as_array().map_or(0, Vec::len);
                notes["count"] = Value::from(total as u64);
                notes["pageInfo"]["hasNextPage"] = Value::Bool(false);
            }
            Err(error) => {
                completion
                    .note_partials
                    .insert(discussion_id, super::PartialReason::PageFailed(error));
            }
        }
    }
    completion
}

/// Read one discussion's complete note list through REST, mapped into the GraphQL node
/// shape (`createdAt`, string ids) so the one discussion parser serves both sources.
fn fetch_full_discussion(
    target: &FetchTarget<'_>,
    iid: u64,
    discussion_id: &str,
) -> Result<Vec<Value>, String> {
    let project = percent_encode(&target.full_path);
    let endpoint = format!("projects/{project}/merge_requests/{iid}/discussions/{discussion_id}");
    let args = ["api", "--hostname", target.host, endpoint.as_str()];
    let out = match super::run_tool(TOOL, target.repo, &args, target.cancelled) {
        Ok(out) => out,
        Err(super::ToolFailure::NoCli(tool)) => return Err(format!("{tool} not found")),
        Err(super::ToolFailure::Io(detail)) => return Err(detail),
        Err(super::ToolFailure::Stderr(stderr)) => return Err(first_line(&stderr)),
    };
    let value: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    let notes = value["notes"].as_array().cloned().unwrap_or_default();
    Ok(notes.iter().map(rest_note_to_node).collect())
}

/// One REST note as a GraphQL-shaped node. Only the fields the discussion parser reads are
/// mapped; the id becomes a plain string so gid-tail comparison dedups across the sources.
fn rest_note_to_node(note: &Value) -> Value {
    serde_json::json!({
        "id": note["id"].as_i64().map_or_else(String::new, |id| id.to_string()),
        "system": note["system"],
        "body": note["body"],
        "createdAt": note["created_at"],
        "resolved": note["resolved"],
        "author": {"username": note["author"]["username"], "bot": false},
    })
}

/// The branch-resolved MR iid (`specs/forge-host.md` "Resolution").
fn resolve_iid(target: &FetchTarget<'_>, input: &PrFetchInput) -> Result<u64, CliError> {
    let resolution = resolve_candidates(target, &input.candidates)?;
    match select_open(&resolution.open, input.head_oid.as_deref()) {
        Pick::One(n) => Ok(n),
        Pick::Ambiguous(count) => Err(CliError::Ambiguous(count)),
        Pick::None => select_historical(&resolution.historical)
            .ok_or_else(|| CliError::NoPr(input.candidates.clone())),
    }
}

/// Map a failed `glab`'s stderr to a degraded state by its wording — like `gh`, `glab` has no
/// stable exit codes for these. A 403 wall counts as unreachable: a self-hosted GitLab commonly
/// IP-allowlists the API (a VPN/proxy matter), while a genuinely forbidden project 404s.
fn classify_failure(stderr: &str, host: &str) -> CliError {
    let s = stderr.to_lowercase();
    if s.contains("glab auth login") || s.contains("not authenticated") || s.contains("401") {
        CliError::NotAuthed { tool: TOOL, host: host.to_owned() }
    } else if super::unreachable_stderr(&s) || s.contains("403") {
        CliError::Unreachable { host: host.to_owned(), detail: first_line(stderr) }
    } else {
        CliError::Other(stderr.trim().to_string())
    }
}

/// The first non-empty line, trimmed — internal-style 403 walls dump whole HTML pages into stderr,
/// and the degraded state needs one readable sentence, not a document.
fn first_line(stderr: &str) -> String {
    stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

struct FetchTarget<'a> {
    repo: &'a Path,
    host: &'a str,
    /// The project's full path including subgroups (`group/subgroup/project`).
    full_path: String,
    cancelled: &'a AtomicBool,
}

/// Which half of the combined resolve response to normalize.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolvePhase {
    Open,
    Historical,
}

struct CandidateResolution {
    open: Vec<Vec<(u64, String)>>,
    historical: Vec<Vec<(u64, String)>>,
}

/// Resolve open and historical MRs for every candidate in one aliased GraphQL call. GitLab's
/// `state` filter takes one state, so each candidate has open, merged, and closed aliases. The
/// slightly larger common response removes a complete `glab` process and network round trip
/// whenever no open MR exists.
fn resolve_candidates(
    target: &FetchTarget<'_>,
    candidates: &[String],
) -> Result<CandidateResolution, CliError> {
    let query = build_resolve_query(candidates.len());
    let mut vars: Vec<(String, String)> = vec![("p".to_string(), target.full_path.clone())];
    for (i, cand) in candidates.iter().enumerate() {
        vars.push((format!("b{i}"), cand.clone()));
    }
    let value = graphql(target, &query, &vars)?;
    Ok(CandidateResolution {
        open: parse_resolve(&value, candidates.len(), ResolvePhase::Open),
        historical: parse_resolve(&value, candidates.len(), ResolvePhase::Historical),
    })
}

/// The fields one discussion node carries, shared by the detail query and the
/// discussion-list pages so a paged discussion can never arrive shaped differently.
const DISCUSSION_NODE: &str = "id notes(first:100){count pageInfo{hasNextPage} \
     nodes{id system body createdAt resolved author{username bot} \
     position{oldPath newPath newLine oldLine}}}";

/// One MR's state in a single direct GraphQL call — identity, mergeability, the head
/// pipeline's jobs, and the discussions. Lists read 100 rows per page; the fetch then walks
/// the discussion cursors to their end (within [`super::PAGE_BUDGET`]) and completes any
/// discussion whose note list is itself capped, so completeness is real rather than inferred.
fn mr_detail(target: &FetchTarget<'_>, iid: u64) -> Result<Value, CliError> {
    // The iid is a parsed u64 inlined into the query text — injection-safe by construction,
    // mirroring the GitHub provider's inlined PR number.
    let q = format!(
        "query($p:ID!){{project(fullPath:$p){{\
         mergeRequest(iid:\"{iid}\"){{\
         iid title webUrl draft state detailedMergeStatus sourceBranch targetBranch \
         diffHeadSha diffRefs{{baseSha startSha headSha}} projectId sourceProjectId \
         headPipeline{{jobs(first:100){{pageInfo{{hasNextPage}} nodes{{name status}}}}}} \
         discussions(first:100){{pageInfo{{hasNextPage endCursor}} nodes{{{DISCUSSION_NODE}}}}}}}}}}}"
    );
    let vars = [("p".to_string(), target.full_path.clone())];
    graphql(target, &q, &vars)
}

/// Run a GraphQL `query` with `vars` through `glab`, host pinned, stderr classified here.
fn graphql(
    target: &FetchTarget<'_>,
    query: &str,
    vars: &[(String, String)],
) -> Result<Value, CliError> {
    super::graphql(TOOL, classify_failure, target.repo, target.host, query, vars, target.cancelled)
}

/// The project's MRs for the picker: open ones, then the newest merged and closed — one call,
/// three aliases (`state` filters take one value). `headPipeline.status` is the CI summary.
pub(crate) fn list_prs(
    repo: &Path,
    target: &crate::git::RepoTarget,
    cancelled: &AtomicBool,
) -> Result<super::PrListing, CliError> {
    const ROW: &str = "nodes{iid title sourceBranch draft state createdAt userNotesCount \
                       resolvableDiscussionsCount resolvedDiscussionsCount author{username} \
                       headPipeline{status}}";
    let q = format!(
        "query($p:ID!){{project(fullPath:$p){{\
         open:mergeRequests(state: opened, first: 100, sort: CREATED_DESC){{{ROW}}} \
         merged:mergeRequests(state: merged, first: 50, sort: CREATED_DESC){{{ROW}}} \
         closed:mergeRequests(state: closed, first: 50, sort: CREATED_DESC){{{ROW}}}}}}}"
    );
    let vars = [("p".to_string(), target.full_path())];
    let v = super::graphql(TOOL, classify_failure, repo, &target.host, &q, &vars, cancelled)?;
    let project = &v["data"]["project"];
    // Merged and closed interleave newest-first: ISO-8601 strings sort lexically.
    let mut done = parse_list(&project["merged"]["nodes"]);
    done.extend(parse_list(&project["closed"]["nodes"]));
    done.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(super::PrListing { open: parse_list(&project["open"]["nodes"]), done })
}

/// Read the first 100 changed files for an explicitly selected MR. The REST diff endpoint
/// reports collapsed/too-large files directly, unlike GitHub's missing-`patch` convention.
pub(crate) fn fetch_review_diff(
    repo: &Path,
    target: &crate::git::RepoTarget,
    number: u64,
    cancelled: &AtomicBool,
) -> Result<PatchSet, CliError> {
    let project = percent_encode(&target.full_path());
    let endpoint = format!("projects/{project}/merge_requests/{number}/diffs?per_page=100");
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
        .ok_or_else(|| CliError::Other("GitLab MR diffs response was not an array".into()))?;
    let files = rows
        .iter()
        .filter_map(|file| {
            let deleted = file["deleted_file"].as_bool().unwrap_or(false);
            let renamed = file["renamed_file"].as_bool().unwrap_or(false);
            let added = file["new_file"].as_bool().unwrap_or(false);
            let old_path = file["old_path"].as_str().unwrap_or_default();
            let new_path = file["new_path"].as_str().unwrap_or(old_path);
            if new_path.is_empty() && old_path.is_empty() {
                return None;
            }
            let patch = file["diff"].as_str().filter(|diff| !diff.is_empty());
            let (additions, deletions) = patch.map(patch_stats).unwrap_or_default();
            Some(PatchFile {
                change: ChangedFile {
                    path: if deleted { old_path } else { new_path }.to_string(),
                    kind: if added {
                        ChangeKind::Added
                    } else if deleted {
                        ChangeKind::Deleted
                    } else if renamed {
                        ChangeKind::Renamed
                    } else {
                        ChangeKind::Modified
                    },
                    additions,
                    deletions,
                    previous_path: renamed.then(|| old_path.to_string()),
                },
                patch: patch.map(str::to_string),
                too_large: file["too_large"].as_bool().unwrap_or(false)
                    || file["collapsed"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    Ok(PatchSet { files, truncated: rows.len() == 100 })
}

fn patch_stats(patch: &str) -> (u32, u32) {
    let additions = patch.lines().filter(|line| line.starts_with('+')).count() as u32;
    let deletions = patch.lines().filter(|line| line.starts_with('-')).count() as u32;
    (additions, deletions)
}

fn percent_encode(value: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

pub(crate) fn sync_review(
    repo: &Path,
    request: &super::ReviewSyncRequest,
    cancelled: &AtomicBool,
) -> super::ReviewSyncOutcome {
    let mut outcome = super::ReviewSyncOutcome::default();
    if request.drafts.is_empty() {
        return outcome;
    }
    let project = percent_encode(&request.target.full_path());
    let base = format!("projects/{project}/merge_requests/{}/draft_notes", request.number);

    // `bulk_publish` publishes every draft note owned by the user. Refuse to absorb drafts
    // created elsewhere into reviewr's group.
    match api_json(repo, &request.target.host, "GET", &base, None, cancelled) {
        Ok(existing) if existing.as_array().is_some_and(Vec::is_empty) => {}
        Ok(_) => {
            let message = "GitLab already has unpublished draft notes — publish or remove them before syncing reviewr".to_string();
            outcome
                .failed
                .extend(request.drafts.iter().map(|draft| (draft.local_id, message.clone())));
            return outcome;
        }
        Err(error) => {
            // The listing is a read: even a network failure cannot have posted a comment.
            let (message, _) = super::sync_failure(error, "GitLab review sync failed");
            record_all(&mut outcome.failed, request, &message);
            return outcome;
        }
    }

    let mut staged: Vec<u64> = Vec::new();
    for draft in &request.drafts {
        let payload = draft_payload(request, draft);
        match api_json(repo, &request.target.host, "POST", &base, Some(&payload), cancelled) {
            Ok(value) => {
                if let Some(id) = value["id"].as_u64() {
                    staged.push(id);
                } else {
                    let cleanup =
                        rollback_drafts(repo, &request.target.host, &base, &staged, cancelled);
                    let message = cleanup_message(
                        "GitLab staged a draft note but returned no id; verify drafts on the forge",
                        &cleanup,
                    );
                    record_all(&mut outcome.uncertain, request, &message);
                    return outcome;
                }
            }
            Err(error) => {
                let (message, outcome_unknown) =
                    super::sync_failure(error, "GitLab draft staging failed");
                let cleanup =
                    rollback_drafts(repo, &request.target.host, &base, &staged, cancelled);
                let message = cleanup_message(&message, &cleanup);
                let target = if outcome_unknown || !cleanup.is_empty() {
                    &mut outcome.uncertain
                } else {
                    &mut outcome.failed
                };
                record_all(target, request, &message);
                return outcome;
            }
        }
    }

    let publish = format!("{base}/bulk_publish");
    match api_json(
        repo,
        &request.target.host,
        "POST",
        &publish,
        Some(&serde_json::json!({})),
        cancelled,
    ) {
        Ok(_) => outcome.succeeded.extend(request.drafts.iter().map(|draft| draft.local_id)),
        Err(error) => {
            let (message, _) = super::sync_failure(error, "GitLab bulk publish failed");
            let cleanup = rollback_drafts(repo, &request.target.host, &base, &staged, cancelled);
            let message = cleanup_message(&message, &cleanup);
            // If every staged note can still be deleted, bulk publish definitely did not accept
            // the group. Any cleanup miss means publication may have succeeded.
            let target =
                if cleanup.is_empty() { &mut outcome.failed } else { &mut outcome.uncertain };
            record_all(target, request, &message);
        }
    }
    outcome
}

fn draft_payload(request: &super::ReviewSyncRequest, draft: &super::ReviewDraft) -> Value {
    match &draft.action {
        super::ReviewDraftAction::Inline(anchor) => {
            let mut position = serde_json::json!({
                "position_type": "text",
                "base_sha": request.diff_refs.base_sha,
                "start_sha": request.diff_refs.start_sha,
                "head_sha": request.diff_refs.head_sha,
                "old_path": anchor.old_path.as_deref().unwrap_or(&anchor.path),
                "new_path": anchor.path,
            });
            match anchor.side {
                crate::model::Side::New => position["new_line"] = anchor.line.into(),
                crate::model::Side::Old => position["old_line"] = anchor.line.into(),
            }
            // A ranged anchor becomes a multi-line note: the top-level position stays the end
            // line (GitLab anchors the thread there) and `line_range` names both endpoints.
            if let (Some(_), Some((start, end))) = (anchor.start_line, anchor.endpoints) {
                position["line_range"] = serde_json::json!({
                    "start": line_range_endpoint(&anchor.path, start),
                    "end": line_range_endpoint(&anchor.path, end),
                });
            }
            serde_json::json!({"note": draft.body, "position": position})
        }
        super::ReviewDraftAction::Reply { remote_id: Some(id), .. } if !id.thread_id.is_empty() => {
            serde_json::json!({
                "note": draft.body,
                "in_reply_to_discussion_id": id.thread_id,
            })
        }
        super::ReviewDraftAction::Reply { author, .. } => {
            serde_json::json!({"note": format!("@{author} {}", draft.body)})
        }
    }
}

/// One `line_range` endpoint, shaped like the GitLab frontend's own payload. GitLab locates
/// the line by `line_code` — `sha1(file_path)_<old>_<new>` over the diff parser's position
/// counters, which exist for every line — and `type` marks how it changed: `new` for an added
/// line, `old` for a removed one, null for context.
fn line_range_endpoint(path: &str, endpoint: crate::diff::RangeEndpoint) -> Value {
    use crate::diff::EndpointKind;
    let digest = sha1_smol::Sha1::from(path).digest().to_string();
    let line_code = format!("{digest}_{}_{}", endpoint.old_pos, endpoint.new_pos);
    let (kind, old_line, new_line) = match endpoint.kind {
        EndpointKind::Added => (Value::from("new"), Value::Null, endpoint.new_pos.into()),
        EndpointKind::Removed => (Value::from("old"), endpoint.old_pos.into(), Value::Null),
        EndpointKind::Context => (Value::Null, endpoint.old_pos.into(), endpoint.new_pos.into()),
    };
    serde_json::json!({
        "line_code": line_code,
        "type": kind,
        "old_line": old_line,
        "new_line": new_line,
    })
}

fn record_all(target: &mut Vec<(u64, String)>, request: &super::ReviewSyncRequest, message: &str) {
    target.extend(request.drafts.iter().map(|draft| (draft.local_id, message.to_string())));
}

fn cleanup_message(message: &str, failed_ids: &[u64]) -> String {
    if failed_ids.is_empty() {
        message.to_string()
    } else {
        format!(
            "{message}; could not remove GitLab draft note(s) {} — verify on the forge",
            failed_ids.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
        )
    }
}

fn rollback_drafts(
    repo: &Path,
    host: &str,
    base: &str,
    staged: &[u64],
    cancelled: &AtomicBool,
) -> Vec<u64> {
    let mut failed = Vec::new();
    for id in staged {
        let endpoint = format!("{base}/{id}");
        if api_json(repo, host, "DELETE", &endpoint, None, cancelled).is_err() {
            failed.push(*id);
        }
    }
    failed
}

fn api_json(
    repo: &Path,
    host: &str,
    method: &str,
    endpoint: &str,
    payload: Option<&Value>,
    cancelled: &AtomicBool,
) -> Result<Value, CliError> {
    let body = payload
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| CliError::Other(error.to_string()))?;
    let args = api_args(host, method, endpoint, body.is_some());
    let out = match super::run_tool_input(TOOL, repo, &args, body.as_deref(), cancelled) {
        Ok(out) => out,
        Err(super::ToolFailure::NoCli(tool)) => return Err(CliError::NoCli(tool)),
        Err(super::ToolFailure::Io(detail)) => return Err(CliError::Other(detail)),
        Err(super::ToolFailure::Stderr(stderr)) => return Err(classify_failure(&stderr, host)),
    };
    if out.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&out).map_err(|error| CliError::Other(error.to_string()))
}

fn api_args<'a>(host: &'a str, method: &'a str, endpoint: &'a str, has_body: bool) -> Vec<&'a str> {
    let mut args = vec!["api", "--hostname", host, "--method", method];
    if has_body {
        // glab 1.105 sends `--input -` without a Content-Type header. GitLab's Draft Notes API
        // rejects that body as HTTP 415, so declare JSON explicitly rather than relying on CLI
        // inference.
        args.extend(["-H", "Content-Type: application/json", "--input", "-"]);
    }
    args.push(endpoint);
    args
}

// ---- Pure normalization (unit-tested) --------------------------------------------------

/// The picker rows from one list alias's `nodes`. A row whose iid does not parse is dropped.
fn parse_list(nodes: &Value) -> Vec<PrListItem> {
    nodes
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|mr| {
            Some(PrListItem {
                number: mr["iid"].as_str()?.parse::<u64>().ok()?,
                title: mr["title"].as_str().unwrap_or_default().to_string(),
                head_ref: mr["sourceBranch"].as_str().unwrap_or_default().to_string(),
                author: mr["author"]["username"].as_str().unwrap_or_default().to_string(),
                is_draft: mr["draft"].as_bool().unwrap_or(false),
                state: parse_state(mr["state"].as_str().unwrap_or("opened")),
                ci: mr["headPipeline"]["status"].as_str().map(job_status),
                created_at: mr["createdAt"].as_str().unwrap_or_default().to_string(),
                comments: mr["userNotesCount"].as_u64().unwrap_or(0) as u32,
                threads_open: resolvable_split(mr).map(|(open, _)| open),
                threads_resolved: resolvable_split(mr).map(|(_, resolved)| resolved),
            })
        })
        .collect()
}

/// The aliased resolve query for `n` candidates: `c{i}` for open MRs and `m{i}`/`x{i}` for
/// the newest merged/closed MR. Keeping all three aliases in one query makes the no-open path
/// one round trip instead of two without changing selection semantics.
fn build_resolve_query(n: usize) -> String {
    use std::fmt::Write;
    let mut q = String::from("query($p:ID!");
    for i in 0..n {
        let _ = write!(q, ",$b{i}:String!");
    }
    q.push_str("){project(fullPath:$p){");
    for i in 0..n {
        let _ = write!(
            q,
            "c{i}:mergeRequests(sourceBranches:[$b{i}], state: opened, first: 100)\
             {{nodes{{iid diffHeadSha}}}} "
        );
        for (alias, state) in [("m", "merged"), ("x", "closed")] {
            let _ = write!(
                q,
                "{alias}{i}:mergeRequests(sourceBranches:[$b{i}], state: {state}, \
                 first: 1, sort: CREATED_DESC){{nodes{{iid createdAt}}}} "
            );
        }
    }
    q.push_str("}}");
    q
}

/// Per-candidate `(iid, extra)` lists from the aliased response. GitLab returns `iid` as a
/// string; a non-numeric or missing iid drops the row. The historical pass concatenates each
/// candidate's merged and closed hits — `select_historical` picks the newest overall.
fn parse_resolve(v: &Value, n: usize, phase: ResolvePhase) -> Vec<Vec<(u64, String)>> {
    let project = &v["data"]["project"];
    let rows = |alias: String, extra: &str| -> Vec<(u64, String)> {
        project[alias.as_str()]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|mr| {
                Some((
                    mr["iid"].as_str()?.parse::<u64>().ok()?,
                    mr[extra].as_str().unwrap_or_default().to_string(),
                ))
            })
            .collect()
    };
    (0..n)
        .map(|i| match phase {
            ResolvePhase::Open => rows(format!("c{i}"), "diffHeadSha"),
            ResolvePhase::Historical => {
                let mut hits = rows(format!("m{i}"), "createdAt");
                hits.extend(rows(format!("x{i}"), "createdAt"));
                hits
            }
        })
        .collect()
}

/// Assemble the snapshot from the MR detail JSON (its discussions already page-walked), the
/// computed `sync`, and the walk's outcome.
fn build_snapshot(node: &Value, sync: Sync, completion: DiscussionCompletion) -> PrSnapshot {
    let jobs = &node["headPipeline"]["jobs"];
    let discussions = &node["discussions"];
    let more = |conn: &Value| conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false);
    // A cross-project MR: the source branch lives in a fork. Either id may be null (deleted
    // fork); only a present-and-different pair marks the fork head.
    let head_is_fork = match (node["sourceProjectId"].as_i64(), node["projectId"].as_i64()) {
        (Some(source), Some(target)) => source != target,
        _ => false,
    };
    PrSnapshot {
        provider: Provider::Gitlab,
        number: node["iid"].as_str().and_then(|iid| iid.parse().ok()).unwrap_or_default(),
        title: node["title"].as_str().unwrap_or_default().to_string(),
        url: node["webUrl"].as_str().unwrap_or_default().to_string(),
        state: parse_state(node["state"].as_str().unwrap_or("opened")),
        is_draft: node["draft"].as_bool().unwrap_or(false),
        head_ref: node["sourceBranch"].as_str().unwrap_or_default().to_string(),
        head_is_fork,
        base_ref: node["targetBranch"].as_str().unwrap_or_default().to_string(),
        diff_refs: super::DiffRefs {
            base_sha: node["diffRefs"]["baseSha"].as_str().unwrap_or_default().to_string(),
            start_sha: node["diffRefs"]["startSha"].as_str().unwrap_or_default().to_string(),
            head_sha: node["diffRefs"]["headSha"].as_str().unwrap_or_default().to_string(),
        },
        merge: derive_merge(node["detailedMergeStatus"].as_str()),
        sync,
        checks: normalize_jobs(&jobs["nodes"]),
        comments: discussion_comments(&discussions["nodes"], &completion.note_partials),
        truncated: more(jobs) || more(discussions),
        threads_partial: completion.threads_partial,
    }
}

/// The (open, resolved) discussion counts, when the row carries the resolvable pair.
fn resolvable_split(mr: &Value) -> Option<(u32, u32)> {
    let resolvable = mr["resolvableDiscussionsCount"].as_u64()? as u32;
    let resolved = mr["resolvedDiscussionsCount"].as_u64().unwrap_or(0) as u32;
    Some((resolvable.saturating_sub(resolved), resolved))
}

/// GitLab's MR lifecycle: `opened`, `merged`, `closed`, and `locked` (mid-transition —
/// reads as closed rather than inventing a fourth state).
fn parse_state(s: &str) -> PrState {
    match s {
        "merged" => PrState::Merged,
        "closed" | "locked" => PrState::Closed,
        _ => PrState::Open,
    }
}

/// Fold GitLab's `detailedMergeStatus` into a [`Merge`]. Same philosophy as GitHub: only
/// actionable blockers surface. `CONFLICT` is the conflict; the review/approval/policy gates
/// are blockers; everything transient or informational (`CHECKING`, `CI_STILL_RUNNING`,
/// `DRAFT_STATUS` — the draft flag is shown separately — `NEED_REBASE`, …) folds into `Clean`.
fn derive_merge(status: Option<&str>) -> Merge {
    match status {
        Some("CONFLICT") => Merge::Conflicting,
        Some(
            "BLOCKED_STATUS"
            | "NOT_APPROVED"
            | "DISCUSSIONS_NOT_RESOLVED"
            | "REQUESTED_CHANGES"
            | "CI_MUST_PASS"
            | "EXTERNAL_STATUS_CHECKS"
            | "SECURITY_POLICIES_VIOLATIONS"
            | "SECURITY_POLICY_PIPELINE_CHECK"
            | "LOCKED_PATHS"
            | "LOCKED_LFS_FILES"
            | "JIRA_ASSOCIATION"
            | "TITLE_NOT_MATCHING",
        ) => Merge::Blocked,
        _ => Merge::Clean,
    }
}

/// The head pipeline's jobs as checks, latest run per job name (a retried job appears once,
/// re-run last in the array).
fn normalize_jobs(nodes: &Value) -> Vec<Check> {
    let mut out: Vec<Check> = Vec::new();
    for node in nodes.as_array().into_iter().flatten() {
        let name = node["name"].as_str().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let status = job_status(node["status"].as_str().unwrap_or(""));
        if let Some(slot) = out.iter_mut().find(|c| c.name == name) {
            *slot = Check { name, status };
        } else {
            out.push(Check { name, status });
        }
    }
    out
}

/// Normalise one `CiJobStatus` to a [`CheckStatus`]. Manual and cancelled jobs read as
/// skipped — they neither fail nor block the rollup, matching GitHub's `NEUTRAL` treatment.
fn job_status(status: &str) -> CheckStatus {
    match status {
        "SUCCESS" => CheckStatus::Success,
        "FAILED" => CheckStatus::Failure,
        "RUNNING" => CheckStatus::Running,
        "SKIPPED" | "MANUAL" | "CANCELED" | "CANCELING" => CheckStatus::Skipped,
        // CREATED / PENDING / PREPARING / SCHEDULED / WAITING_* — queued in some form.
        _ => CheckStatus::Pending,
    }
}

/// GitLab has one comment surface: discussions. Each discussion's root note becomes a card —
/// a positioned note is an inline `Finding` (`path:line` anchor), an unpositioned one a plain
/// `Comment`. System notes (approvals, pushes, label churn) are noise, not review content.
/// GitLab marks bots first-class (`author.bot`), so a bot's prose collapses to its latest
/// exactly as on GitHub. `note_partials` names the discussions whose note completion failed,
/// so their prefix is marked with the failure rather than a generic cap.
fn discussion_comments(
    discussions: &Value,
    note_partials: &std::collections::HashMap<String, super::PartialReason>,
) -> Vec<Comment> {
    let mut out: Vec<Comment> = Vec::new();
    for discussion in discussions.as_array().into_iter().flatten() {
        let notes = &discussion["notes"];
        let root = &notes["nodes"][0];
        if root["system"].as_bool().unwrap_or(false) {
            continue;
        }
        let body = root["body"].as_str().unwrap_or("").trim().to_string();
        if body.is_empty() {
            continue;
        }
        let position = &root["position"];
        let path = position["newPath"]
            .as_str()
            .or_else(|| position["oldPath"].as_str())
            .or_else(|| position["filePath"].as_str());
        let diff_anchor = path.and_then(|path| {
            let (side, line) =
                position["newLine"].as_u64().map(|line| (crate::model::Side::New, line)).or_else(
                    || position["oldLine"].as_u64().map(|line| (crate::model::Side::Old, line)),
                )?;
            Some(super::DiffAnchor {
                path: path.to_string(),
                old_path: position["oldPath"].as_str().map(str::to_string),
                side,
                line: line as u32,
                // GitLab's GraphQL `DiffPosition` exposes no line range, so a multi-line
                // discussion reads back anchored to its end line only.
                start_line: None,
                endpoints: None,
            })
        });
        let (kind, anchor) = match &diff_anchor {
            Some(anchor) => (CommentKind::Finding, anchor.location()),
            None => (CommentKind::Comment, "comment".to_string()),
        };
        let mut replies: Vec<super::RemoteReply> = notes["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(1)
            .filter(|note| !note["system"].as_bool().unwrap_or(false))
            .map(|note| super::RemoteReply {
                id: gid_tail(note["id"].as_str().unwrap_or_default()).to_string(),
                author: note["author"]["username"].as_str().unwrap_or_default().to_string(),
                body: note["body"].as_str().unwrap_or_default().to_string(),
                created_at: note["createdAt"].as_str().unwrap_or_default().to_string(),
            })
            .collect();
        super::sort_replies(&mut replies);
        // A surviving next-page flag means the note walk did not finish: the chain is a
        // prefix, marked with the recorded failure or the page cap. `missing` counts notes
        // (`count` includes system notes), and never reads zero while pages remain.
        let discussion_gid = discussion["id"].as_str().unwrap_or_default().to_string();
        let loaded = notes["nodes"].as_array().map_or(0, Vec::len) as u32;
        let replies_state = if notes["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
            let total = notes["count"].as_u64().unwrap_or(0) as u32;
            super::RepliesState::Partial {
                missing: total.saturating_sub(loaded).max(1),
                reason: note_partials
                    .get(&discussion_gid)
                    .cloned()
                    .unwrap_or(super::PartialReason::Capped),
            }
        } else {
            super::RepliesState::Complete
        };
        out.push(Comment {
            kind,
            author: root["author"]["username"].as_str().unwrap_or("").to_string(),
            author_is_bot: root["author"]["bot"].as_bool().unwrap_or(false),
            anchor,
            body,
            snippet: None,
            created_at: root["createdAt"].as_str().unwrap_or("").to_string(),
            is_resolved: root["resolved"].as_bool().unwrap_or(false),
            is_outdated: false,
            reply_count: notes["count"].as_u64().unwrap_or(1).saturating_sub(1) as u32,
            replies,
            replies_state,
            diff_anchor,
            remote_id: Some(super::RemoteCommentId {
                thread_id: gid_tail(&discussion_gid).to_string(),
                root_comment_id: gid_tail(root["id"].as_str().unwrap_or_default()).parse().ok(),
            }),
        });
    }
    super::dedup_bot_prose(&mut out);
    // Newest first: ISO-8601 strings sort lexically in chronological order.
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

fn gid_tail(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_payloads_keep_gitlab_group_positions_and_thread_ids() {
        let request = super::super::ReviewSyncRequest::new(
            crate::git::RepoTarget {
                provider: Provider::Gitlab,
                host: "gitlab.com".into(),
                owner: "group".into(),
                name: "project".into(),
            },
            9,
            super::super::DiffRefs {
                base_sha: "base".into(),
                start_sha: "start".into(),
                head_sha: "head".into(),
            },
            Vec::new(),
        );
        let inline = super::super::ReviewDraft {
            local_id: 1,
            action: super::super::ReviewDraftAction::Inline(super::super::DiffAnchor {
                path: "new.rs".into(),
                old_path: Some("old.rs".into()),
                side: crate::model::Side::New,
                line: 12,
                start_line: None,
                endpoints: None,
            }),
            body: "inline".into(),
        };
        let payload = draft_payload(&request, &inline);
        assert_eq!(payload["position"]["base_sha"], "base");
        assert_eq!(payload["position"]["old_path"], "old.rs");
        assert_eq!(payload["position"]["new_line"], 12);
        assert!(
            payload["position"].get("line_range").is_none(),
            "a single-line note carries no line_range"
        );

        let reply = super::super::ReviewDraft {
            local_id: 2,
            action: super::super::ReviewDraftAction::Reply {
                remote_id: Some(super::super::RemoteCommentId {
                    thread_id: "discussion".into(),
                    root_comment_id: Some(1),
                }),
                author: "reviewer".into(),
            },
            body: "reply".into(),
        };
        assert_eq!(draft_payload(&request, &reply)["in_reply_to_discussion_id"], "discussion");
    }

    #[test]
    fn a_ranged_draft_becomes_a_multi_line_note_with_frontend_shaped_line_codes() {
        use crate::diff::{EndpointKind, RangeEndpoint};
        let request = super::super::ReviewSyncRequest::new(
            crate::git::RepoTarget {
                provider: Provider::Gitlab,
                host: "gitlab.com".into(),
                owner: "group".into(),
                name: "project".into(),
            },
            9,
            super::super::DiffRefs {
                base_sha: "base".into(),
                start_sha: "start".into(),
                head_sha: "head".into(),
            },
            Vec::new(),
        );
        // A range from a context line (both counters are line numbers) to an added line
        // (its old counter is the next unconsumed old line, and its old_line is null).
        let ranged = super::super::ReviewDraft {
            local_id: 3,
            action: super::super::ReviewDraftAction::Inline(super::super::DiffAnchor {
                path: "new.rs".into(),
                old_path: None,
                side: crate::model::Side::New,
                line: 14,
                start_line: Some(12),
                endpoints: Some((
                    RangeEndpoint { old_pos: 12, new_pos: 12, kind: EndpointKind::Context },
                    RangeEndpoint { old_pos: 13, new_pos: 14, kind: EndpointKind::Added },
                )),
            }),
            body: "range".into(),
        };
        let payload = draft_payload(&request, &ranged);
        // The thread still anchors to the end line; the range rides beside it.
        assert_eq!(payload["position"]["new_line"], 14);
        let range = &payload["position"]["line_range"];
        // sha1("new.rs") — the line code addresses both parser counters.
        let sha = "e6d049e7e635307aff069ed13eadd2e56f36bc52";
        assert_eq!(range["start"]["line_code"], format!("{sha}_12_12"));
        assert_eq!(range["start"]["type"], Value::Null);
        assert_eq!(range["start"]["old_line"], 12);
        assert_eq!(range["start"]["new_line"], 12);
        assert_eq!(range["end"]["line_code"], format!("{sha}_13_14"));
        assert_eq!(range["end"]["type"], "new");
        assert_eq!(range["end"]["old_line"], Value::Null);
        assert_eq!(range["end"]["new_line"], 14);
    }

    #[test]
    fn json_api_requests_declare_their_media_type() {
        let args = api_args("gitlab.example.com", "POST", "draft_notes", true);
        assert!(
            args.windows(2).any(|pair| pair == ["-H", "Content-Type: application/json"]),
            "glab --input does not infer Content-Type, causing GitLab HTTP 415"
        );
    }

    #[test]
    fn rollback_errors_name_every_server_draft_that_needs_manual_cleanup() {
        assert_eq!(cleanup_message("publish failed", &[]), "publish failed");
        assert_eq!(
            cleanup_message("publish failed", &[17, 23]),
            "publish failed; could not remove GitLab draft note(s) 17, 23 — verify on the forge"
        );
    }

    #[test]
    fn mr_diffs_normalize_subgroup_paths_flags_and_patch_stats() {
        assert_eq!(percent_encode("group/sub/project"), "group%2Fsub%2Fproject");
        let value = serde_json::json!([
            {
                "old_path": "src/old.rs", "new_path": "src/new.rs",
                "new_file": false, "deleted_file": false, "renamed_file": true,
                "collapsed": false, "too_large": false,
                "diff": "@@ -1 +1,2 @@\n-old\n+new\n+more"
            },
            {
                "old_path": "asset.bin", "new_path": "asset.bin",
                "new_file": false, "deleted_file": false, "renamed_file": false,
                "collapsed": true, "too_large": false, "diff": ""
            }
        ]);
        let patch = parse_review_diff(&value).unwrap();
        assert_eq!(patch.files[0].change.kind, ChangeKind::Renamed);
        assert_eq!(patch.files[0].change.previous_path.as_deref(), Some("src/old.rs"));
        assert_eq!((patch.files[0].change.additions, patch.files[0].change.deletions), (2, 1));
        assert!(patch.files[1].too_large);
        assert!(patch.files[1].patch.is_none());
    }

    #[test]
    fn resolve_query_combines_every_state_and_never_inlines_names() {
        let query = build_resolve_query(2);
        assert!(query.starts_with("query($p:ID!,$b0:String!,$b1:String!){project(fullPath:$p){"));
        for i in 0..2 {
            assert!(query.contains(&format!(
                "c{i}:mergeRequests(sourceBranches:[$b{i}], state: opened, first: 100){{nodes{{iid diffHeadSha}}}}"
            )));
            assert!(query.contains(&format!(
                "m{i}:mergeRequests(sourceBranches:[$b{i}], state: merged, first: 1, sort: CREATED_DESC){{nodes{{iid createdAt}}}}"
            )));
            assert!(query.contains(&format!(
                "x{i}:mergeRequests(sourceBranches:[$b{i}], state: closed, first: 1, sort: CREATED_DESC){{nodes{{iid createdAt}}}}"
            )));
        }
        assert!(!query.contains("feature/name"));
    }

    #[test]
    fn parse_resolve_reads_string_iids_and_merges_historical_aliases() {
        let open = serde_json::json!({"data": {"project": {
            "c0": {"nodes": [{"iid": "7", "diffHeadSha": "abc"}]},
            "c1": null,
            "c2": {"nodes": [{"iid": "9", "diffHeadSha": "def"}, {"iid": "not-a-number", "diffHeadSha": "ghi"}]}
        }}});
        let per = parse_resolve(&open, 3, ResolvePhase::Open);
        assert_eq!(per[0], [(7, "abc".to_string())]);
        assert!(per[1].is_empty());
        assert_eq!(per[2], [(9, "def".to_string())]); // the unparsable iid dropped

        let hist = serde_json::json!({"data": {"project": {
            "m0": {"nodes": [{"iid": "3", "createdAt": "2026-06-01T00:00:00Z"}]},
            "x0": {"nodes": [{"iid": "5", "createdAt": "2026-06-04T00:00:00Z"}]}
        }}});
        let per = parse_resolve(&hist, 1, ResolvePhase::Historical);
        assert_eq!(
            per[0],
            [(3, "2026-06-01T00:00:00Z".to_string()), (5, "2026-06-04T00:00:00Z".to_string())]
        );
    }

    #[test]
    fn merge_surfaces_only_conflicts_and_actionable_blockers() {
        assert_eq!(derive_merge(Some("CONFLICT")), Merge::Conflicting);
        assert_eq!(derive_merge(Some("BLOCKED_STATUS")), Merge::Blocked);
        assert_eq!(derive_merge(Some("NOT_APPROVED")), Merge::Blocked);
        assert_eq!(derive_merge(Some("DISCUSSIONS_NOT_RESOLVED")), Merge::Blocked);
        assert_eq!(derive_merge(Some("CI_MUST_PASS")), Merge::Blocked);
        // Transient or informational states fold into Clean.
        assert_eq!(derive_merge(Some("MERGEABLE")), Merge::Clean);
        assert_eq!(derive_merge(Some("CHECKING")), Merge::Clean);
        assert_eq!(derive_merge(Some("CI_STILL_RUNNING")), Merge::Clean);
        assert_eq!(derive_merge(Some("DRAFT_STATUS")), Merge::Clean); // draft is its own marker
        assert_eq!(derive_merge(Some("NEED_REBASE")), Merge::Clean); // GitHub BEHIND analogue
        assert_eq!(derive_merge(None), Merge::Clean);
    }

    #[test]
    fn state_maps_the_gitlab_lifecycle_with_locked_as_closed() {
        assert_eq!(parse_state("opened"), PrState::Open);
        assert_eq!(parse_state("merged"), PrState::Merged);
        assert_eq!(parse_state("closed"), PrState::Closed);
        assert_eq!(parse_state("locked"), PrState::Closed);
        assert_eq!(parse_state("anything-else"), PrState::Open);
    }

    #[test]
    fn job_statuses_normalise_to_check_statuses() {
        assert_eq!(job_status("SUCCESS"), CheckStatus::Success);
        assert_eq!(job_status("FAILED"), CheckStatus::Failure);
        assert_eq!(job_status("RUNNING"), CheckStatus::Running);
        assert_eq!(job_status("PENDING"), CheckStatus::Pending);
        assert_eq!(job_status("CREATED"), CheckStatus::Pending);
        assert_eq!(job_status("WAITING_FOR_RESOURCE"), CheckStatus::Pending);
        assert_eq!(job_status("SKIPPED"), CheckStatus::Skipped);
        assert_eq!(job_status("MANUAL"), CheckStatus::Skipped);
        assert_eq!(job_status("CANCELED"), CheckStatus::Skipped);
    }

    #[test]
    fn snapshot_reads_mr_identity_fork_and_truncation() {
        let node = serde_json::json!({
            "iid": "42", "title": "Add GitLab support", "webUrl": "https://gitlab.example.com/g/p/-/merge_requests/42",
            "draft": true, "state": "opened", "detailedMergeStatus": "MERGEABLE",
            "sourceBranch": "feature/gitlab", "targetBranch": "main", "diffHeadSha": "abc",
            "projectId": 100, "sourceProjectId": 200,
            "headPipeline": {"jobs": {"pageInfo": {"hasNextPage": true}, "nodes": [
                {"name": "test", "status": "SUCCESS"},
                {"name": "build", "status": "RUNNING"}
            ]}},
            "discussions": {"pageInfo": {"hasNextPage": false}, "nodes": []}
        });
        let s = build_snapshot(&node, Sync::InSync, DiscussionCompletion::default());
        assert_eq!(s.provider, Provider::Gitlab);
        assert_eq!(s.number, 42);
        assert!(s.is_draft);
        assert_eq!(s.head_ref, "feature/gitlab");
        assert_eq!(s.base_ref, "main");
        assert!(s.head_is_fork, "differing project ids mark a fork MR");
        assert_eq!(s.checks.len(), 2);
        assert!(s.truncated, "a paging jobs list marks the snapshot truncated");

        // A same-project MR is not a fork; absent pipeline degrades soft.
        let bare = serde_json::json!({
            "iid": "7", "projectId": 100, "sourceProjectId": 100, "state": "merged",
            "discussions": {"nodes": []}
        });
        let s = build_snapshot(&bare, Sync::Unknown, DiscussionCompletion::default());
        assert!(!s.head_is_fork);
        assert_eq!(s.state, PrState::Merged);
        assert!(s.checks.is_empty());
        assert!(!s.truncated);
    }

    #[test]
    fn discussions_become_findings_and_comments_skipping_system_notes() {
        let nodes = serde_json::json!([
            {"notes": {"count": 3, "nodes": [{
                "system": false, "body": "off-by-one here",
                "createdAt": "2026-07-01T10:00:00Z", "resolved": true,
                "author": {"username": "yassin17", "bot": false},
                "position": {"filePath": "src/a.rs", "newLine": 12, "oldLine": null}
            }]}},
            {"notes": {"count": 1, "nodes": [{
                "system": false, "body": "LGTM overall",
                "createdAt": "2026-07-01T12:00:00Z", "resolved": null,
                "author": {"username": "reviewer", "bot": false},
                "position": null
            }]}},
            {"notes": {"count": 1, "nodes": [{
                "system": true, "body": "approved this merge request",
                "createdAt": "2026-07-01T13:00:00Z",
                "author": {"username": "reviewer", "bot": false}
            }]}}
        ]);
        let cs = discussion_comments(&nodes, &std::collections::HashMap::new());
        assert_eq!(cs.len(), 2, "the system note is dropped");
        // Newest first.
        assert_eq!(cs[0].kind, CommentKind::Comment);
        assert_eq!(cs[0].anchor, "comment");
        assert_eq!(cs[1].kind, CommentKind::Finding);
        assert_eq!(cs[1].anchor, "src/a.rs:12");
        assert!(cs[1].is_resolved);
        assert_eq!(cs[1].reply_count, 2);
    }

    #[test]
    fn rest_notes_map_to_graphql_shape_and_dedup_by_gid_tail() {
        // The first GraphQL page already holds the root and one reply (gid ids); the REST
        // completion returns the whole note set (integer ids). Appending must not duplicate
        // the overlap, and the merged chain must read chronologically.
        let mut notes = serde_json::json!({"count": 3, "pageInfo": {"hasNextPage": true},
        "nodes": [
            {"id": "gid://gitlab/DiscussionNote/1", "system": false, "body": "root",
             "createdAt": "2026-07-01T10:00:00Z", "resolved": false,
             "author": {"username": "root", "bot": false}, "position": null},
            {"id": "gid://gitlab/DiscussionNote/2", "system": false, "body": "first reply",
             "createdAt": "2026-07-01T11:00:00Z",
             "author": {"username": "a", "bot": false}}
        ]});
        let rest = serde_json::json!([
            {"id": 1, "system": false, "body": "root", "created_at": "2026-07-01T10:00:00Z",
             "resolved": false, "author": {"username": "root"}},
            {"id": 2, "system": false, "body": "first reply", "created_at": "2026-07-01T11:00:00Z",
             "author": {"username": "a"}},
            {"id": 3, "system": false, "body": "second reply", "created_at": "2026-07-01T12:00:00Z",
             "author": {"username": "b"}}
        ]);
        let mapped: Vec<_> =
            rest.as_array().unwrap().iter().map(super::rest_note_to_node).collect();
        super::super::append_nodes_by_key(&mut notes, mapped, |node| {
            node["id"].as_str().map(|id| super::gid_tail(id).to_string())
        });
        let merged = notes["nodes"].as_array().unwrap();
        assert_eq!(merged.len(), 3, "overlapping notes dedup by gid tail");
        assert_eq!(merged[2]["body"], "second reply");
        assert_eq!(merged[2]["createdAt"], "2026-07-01T12:00:00Z", "REST fields map to GraphQL");

        // Parsed with the completed flag cleared, the discussion reads complete.
        notes["pageInfo"]["hasNextPage"] = serde_json::Value::Bool(false);
        let discussions = serde_json::json!([{ "id": "gid://gitlab/Discussion/abc",
            "notes": notes }]);
        let cs = discussion_comments(&discussions, &std::collections::HashMap::new());
        assert_eq!(cs[0].replies_state, super::super::RepliesState::Complete);
        let order: Vec<_> = cs[0].replies.iter().map(|r| r.body.as_str()).collect();
        assert_eq!(order, ["first reply", "second reply"]);
    }

    #[test]
    fn an_unfinished_note_walk_marks_the_discussion_partial() {
        let discussions = serde_json::json!([{ "id": "gid://gitlab/Discussion/abc",
        "notes": {"count": 9, "pageInfo": {"hasNextPage": true}, "nodes": [
            {"id": "gid://gitlab/DiscussionNote/1", "system": false, "body": "root",
             "createdAt": "2026-07-01T10:00:00Z", "resolved": false,
             "author": {"username": "root", "bot": false}, "position": null},
            {"id": "gid://gitlab/DiscussionNote/2", "system": false, "body": "reply",
             "createdAt": "2026-07-01T11:00:00Z",
             "author": {"username": "a", "bot": false}}
        ]}}]);
        // Without a recorded failure: the page cap. Missing counts notes, never zero.
        let cs = discussion_comments(&discussions, &std::collections::HashMap::new());
        assert_eq!(
            cs[0].replies_state,
            super::super::RepliesState::Partial {
                missing: 7,
                reason: super::super::PartialReason::Capped
            }
        );
        // A recorded REST failure names itself, keyed by the full discussion gid.
        let mut partials = std::collections::HashMap::new();
        partials.insert(
            "gid://gitlab/Discussion/abc".to_string(),
            super::super::PartialReason::PageFailed("HTTP 502".into()),
        );
        let cs = discussion_comments(&discussions, &partials);
        match &cs[0].replies_state {
            super::super::RepliesState::Partial {
                reason: super::super::PartialReason::PageFailed(message),
                ..
            } => assert!(message.contains("HTTP 502")),
            other => panic!("expected PageFailed, got {other:?}"),
        }
    }

    #[test]
    fn a_gitlab_bots_prose_collapses_to_its_latest() {
        let nodes = serde_json::json!([
            {"notes": {"count": 1, "nodes": [{
                "system": false, "body": "old bot summary", "createdAt": "2026-07-01T09:00:00Z",
                "author": {"username": "duo_bot", "bot": true}, "position": null
            }]}},
            {"notes": {"count": 1, "nodes": [{
                "system": false, "body": "new bot summary", "createdAt": "2026-07-01T11:00:00Z",
                "author": {"username": "duo_bot", "bot": true}, "position": null
            }]}}
        ]);
        let cs = discussion_comments(&nodes, &std::collections::HashMap::new());
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].body, "new bot summary");
    }

    #[test]
    fn mr_list_rows_parse_state_pipeline_and_drop_bad_iids() {
        let nodes = serde_json::json!([
            {"iid": "42", "title": "GitLab support", "sourceBranch": "feat/mr", "draft": false,
             "state": "merged", "createdAt": "2026-07-01T00:00:00Z", "userNotesCount": 7,
             "resolvableDiscussionsCount": 5, "resolvedDiscussionsCount": 3,
             "author": {"username": "yassin17"}, "headPipeline": {"status": "SUCCESS"}},
            {"iid": "oops", "title": "bad"},
            {"iid": "7", "title": "Broken", "sourceBranch": "wip", "draft": true,
             "state": "closed", "createdAt": "2026-06-01T00:00:00Z",
             "author": null, "headPipeline": {"status": "FAILED"}}
        ]);
        let items = parse_list(&nodes);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].state, PrState::Merged);
        assert_eq!(items[0].ci, Some(CheckStatus::Success));
        assert_eq!(items[0].comments, 7);
        assert_eq!(items[0].threads_open, Some(2), "resolvable minus resolved");
        assert_eq!(items[0].threads_resolved, Some(3));
        assert_eq!(items[1].ci, Some(CheckStatus::Failure));
        // No pipeline at all → no CI verdict.
        let bare = serde_json::json!([{"iid": "1", "state": "opened", "headPipeline": null}]);
        assert_eq!(parse_list(&bare)[0].ci, None);
    }

    #[test]
    fn glab_failure_classifies_by_stderr_wording() {
        assert_eq!(
            classify_failure(
                "To get started with GitLab CLI, please run: glab auth login",
                "gitlab.com"
            ),
            CliError::NotAuthed { tool: "glab", host: "gitlab.com".to_string() }
        );
        // The internal-style IP-allowlist wall: HTTP 403 with an HTML page — unreachable, and the
        // detail collapses to the first readable line, not the whole document.
        let wall = classify_failure(
            "403 Forbidden - You are not allowed to access this page.\n<html>\n...",
            "gitlab.selfhosted.example.com",
        );
        assert_eq!(
            wall,
            CliError::Unreachable {
                host: "gitlab.selfhosted.example.com".to_string(),
                detail: "403 Forbidden - You are not allowed to access this page.".to_string()
            }
        );
        assert_eq!(
            classify_failure("HTTP 500 something", "gitlab.com"),
            CliError::Other("HTTP 500 something".into())
        );
    }
}
