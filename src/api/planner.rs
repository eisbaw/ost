//! Microsoft Planner plans, buckets, and tasks (Graph Planner API).
//!
//! OstMac (om-planner): the Planner tab reads a team's task boards —
//! plans per team (group), buckets + tasks per plan — over the same
//! Graph token as the rest of the app. No scope widening is attempted;
//! a 403 surfaces as the call's detail (same contract as om-remind).
//!
//! Docs grounding (Microsoft Graph v1.0):
//! - list plans: `GET /groups/{id}/planner/plans`
//! - list buckets: `GET /planner/plans/{id}/buckets`
//! - list tasks: `GET /planner/plans/{id}/tasks`
//! - create task: `POST /planner/tasks` (`planId`, `bucketId`, `title`)
//! - update task: `PATCH /planner/tasks/{id}` with **required** `If-Match`
//!   carrying the task's last known `@odata.etag`
//!   (https://learn.microsoft.com/en-us/graph/api/plannertask-update).
//!   Completion is `percentComplete`: 100 done, 0 reopened.
//!   Assignment is `assignments`: `{userId: plannerAssignment}` adds,
//!   `{userId: null}` removes
//!   (https://learn.microsoft.com/en-us/graph/api/resources/plannerassignments).
//!   The PATCH sends `Prefer: return=representation`; without it Graph
//!   answers 204 with no body and there is no task to parse.
//!
//! The `If-Match` PATCH goes through
//! [`TeamsClient::graph_patch_etag`]; `DELETE` with `If-Match` goes
//! through [`TeamsClient::graph_delete_etag`]. Both ride the client's
//! token + response check, so no cached-config fallback is needed.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

#[derive(Debug, Deserialize)]
struct PlansResponse {
    value: Vec<Plan>,
}

#[derive(Debug, Deserialize)]
struct Plan {
    id: String,
    title: Option<String>,
    #[serde(rename = "categoryDescriptions")]
    categories: Option<std::collections::BTreeMap<String, Option<String>>>,
    #[serde(rename = "@odata.etag")]
    etag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BucketsResponse {
    value: Vec<Bucket>,
}

#[derive(Debug, Deserialize)]
struct Bucket {
    id: String,
    name: Option<String>,
    #[serde(rename = "planId")]
    plan_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TasksResponse {
    value: Vec<PlannerTask>,
}

#[derive(Debug, Deserialize)]
struct PlannerTask {
    id: String,
    #[serde(rename = "planId")]
    plan_id: Option<String>,
    #[serde(rename = "bucketId")]
    bucket_id: Option<String>,
    title: Option<String>,
    #[serde(rename = "percentComplete")]
    percent_complete: Option<i32>,
    priority: Option<i32>,
    #[serde(rename = "dueDateTime")]
    due_date_time: Option<String>,
    #[serde(rename = "@odata.etag")]
    etag: Option<String>,
    /// `assignments`: user id -> plannerAssignment (null = removed).
    assignments: Option<serde_json::Map<String, serde_json::Value>>,
    /// Set labels: `{"categoryN": true}` (N = 1..=6).
    #[serde(rename = "appliedCategories")]
    applied_categories: Option<serde_json::Map<String, serde_json::Value>>,
    /// Group conversation thread backing Planner comments (null until
    /// the first comment exists).
    #[serde(rename = "conversationThreadId")]
    thread_id: Option<String>,
}

/// Reject ids that would break out of the Graph path segment.
fn check_id(what: &str, id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("empty {}", what);
    }
    if id.contains('/')
        || id.contains('?')
        || id.contains('#')
        || id.chars().any(|c| c.is_whitespace())
    {
        bail!("{} must not contain '/', '?', '#' or whitespace", what);
    }
    Ok(())
}

/// An etag travels back verbatim into the `If-Match` header; it must be
/// a single header line.
fn check_etag(etag: &str) -> Result<()> {
    if etag.trim().is_empty() {
        bail!("empty etag");
    }
    if etag.chars().any(|c| c == '\r' || c == '\n') {
        bail!("etag must not contain CR or LF");
    }
    Ok(())
}

/// `GET` path for a team's (group's) plans. A team's id is its group id.
pub fn plans_path(group_id: &str) -> Result<String> {
    check_id("group_id", group_id)?;
    Ok(format!("/groups/{}/planner/plans", group_id))
}

/// `GET` path for one plan's buckets.
pub fn buckets_path(plan_id: &str) -> Result<String> {
    check_id("plan_id", plan_id)?;
    Ok(format!("/planner/plans/{}/buckets", plan_id))
}

/// `GET` path for one plan's tasks.
pub fn tasks_path(plan_id: &str, limit: usize) -> Result<String> {
    check_id("plan_id", plan_id)?;
    Ok(format!("/planner/plans/{}/tasks?$top={}", plan_id, limit))
}

/// `PATCH` path for one task.
pub fn task_path(task_id: &str) -> Result<String> {
    check_id("task_id", task_id)?;
    Ok(format!("/planner/tasks/{}", task_id))
}

/// Update body: 100 completes, 0 reopens (docs: `percentComplete`).
pub fn set_complete_body(complete: bool) -> serde_json::Value {
    serde_json::json!({ "percentComplete": if complete { 100 } else { 0 } })
}

/// Assign (`assign=true`) or unassign one user. The id becomes a JSON
/// key, so it gets the same guard as a path id.
pub fn set_assignment_body(user_id: &str, assign: bool) -> Result<serde_json::Value> {
    check_id("user_id", user_id)?;
    let value = if assign {
        serde_json::json!({
            "@odata.type": "#microsoft.graph.plannerAssignment",
            "orderHint": " !",
        })
    } else {
        serde_json::Value::Null
    };
    let mut map = serde_json::Map::new();
    map.insert(user_id.to_string(), value);
    Ok(serde_json::json!({ "assignments": map }))
}

/// Create-task body. Empty titles are rejected before any network.
pub fn create_task_body(plan_id: &str, bucket_id: &str, title: &str) -> Result<serde_json::Value> {
    check_id("plan_id", plan_id)?;
    check_id("bucket_id", bucket_id)?;
    if title.trim().is_empty() {
        bail!("empty title");
    }
    Ok(serde_json::json!({
        "planId": plan_id,
        "bucketId": bucket_id,
        "title": title,
    }))
}

// ---------------------------------------------------------------------------
// Data-returning API functions for embedders
// ---------------------------------------------------------------------------

/// Planner board metadata.
pub struct PlanInfo {
    pub id: String,
    pub title: String,
    /// Named labels, `(1..=6, name)`, sorted; empty when unnamed.
    pub categories: Vec<(u8, String)>,
    /// `@odata.etag` verbatim; plan DELETE sends it back as `If-Match`.
    pub etag: String,
}

/// Bucket (board column) metadata.
pub struct BucketInfo {
    pub id: String,
    pub plan_id: String,
    pub name: String,
}

/// Planner task metadata.
pub struct PlannerTaskInfo {
    pub id: String,
    pub plan_id: String,
    pub bucket_id: String,
    pub title: String,
    /// 0-100 from Graph (`percentComplete`); missing reads as 0.
    pub percent_complete: i32,
    /// True when `percent_complete` is 100.
    pub completed: bool,
    /// 0-10 from Graph, passed through when present.
    pub priority: Option<i32>,
    /// `dueDateTime` verbatim, when set.
    pub due: Option<String>,
    /// `@odata.etag` verbatim; required for complete/reopen/assign.
    pub etag: String,
    /// Assigned user ids (`assignments` keys with a non-null value),
    /// sorted; empty when unassigned.
    pub assignee_ids: Vec<String>,
    /// Set labels (`appliedCategories` keys `categoryN` with a truthy
    /// value), sorted; empty when unlabeled.
    pub applied_labels: Vec<u8>,
    /// Group conversation thread backing Planner comments; empty until
    /// the first comment exists.
    pub thread_id: String,
}

fn plan_info(p: Plan) -> PlanInfo {
    let title = p
        .title
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| p.id.clone());
    PlanInfo {
        id: p.id,
        title,
        categories: category_names(p.categories),
        etag: p.etag.unwrap_or_default(),
    }
}

/// Named labels from `categoryDescriptions`: `{"categoryN": name}`
/// entries with a non-blank name, N parsed 1..=6, sorted.
fn category_names(
    map: Option<std::collections::BTreeMap<String, Option<String>>>,
) -> Vec<(u8, String)> {
    let mut out: Vec<(u8, String)> = map
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| {
            let n: u8 = k.strip_prefix("category")?.parse().ok()?;
            if !(1..=6).contains(&n) {
                return None;
            }
            let name = v.filter(|s| !s.trim().is_empty())?;
            (!name.trim().is_empty()).then_some((n, name))
        })
        .collect();
    out.sort();
    out
}

fn bucket_info(b: Bucket) -> BucketInfo {
    let name = b
        .name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| b.id.clone());
    BucketInfo {
        id: b.id,
        plan_id: b.plan_id.unwrap_or_default(),
        name,
    }
}

fn task_info(t: PlannerTask) -> PlannerTaskInfo {
    let percent = t.percent_complete.unwrap_or(0);
    PlannerTaskInfo {
        id: t.id,
        plan_id: t.plan_id.unwrap_or_default(),
        bucket_id: t.bucket_id.unwrap_or_default(),
        title: t.title.unwrap_or_default(),
        completed: percent == 100,
        percent_complete: percent,
        priority: t.priority,
        due: t.due_date_time,
        etag: t.etag.unwrap_or_default(),
        assignee_ids: assignee_ids(t.assignments),
        applied_labels: applied_labels(t.applied_categories),
        thread_id: t.thread_id.filter(|s| !s.trim().is_empty()).unwrap_or_default(),
    }
}

/// `appliedCategories` keys `categoryN` (1..=6) with a truthy value.
/// Sorted; a null/false value is a cleared label, never a set one.
fn applied_labels(map: Option<serde_json::Map<String, serde_json::Value>>) -> Vec<u8> {
    let mut out: Vec<u8> = map
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| {
            let n: u8 = k.strip_prefix("category")?.parse().ok()?;
            if !(1..=6).contains(&n) {
                return None;
            }
            v.as_bool().filter(|b| *b).map(|_| n)
        })
        .collect();
    out.sort();
    out
}

/// `assignments` keys whose value is an object (a null value is a
/// removal echo, never an assignee). Sorted for stable output.
fn assignee_ids(map: Option<serde_json::Map<String, serde_json::Value>>) -> Vec<String> {
    let mut ids: Vec<String> = map
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, v)| !k.trim().is_empty() && !v.is_null())
        .map(|(k, _)| k)
        .collect();
    ids.sort();
    ids
}

/// Parse a plans collection body (pure; powers tests + embedders).
pub fn parse_plans(body: &[u8]) -> Result<Vec<PlanInfo>> {
    let resp: PlansResponse =
        serde_json::from_slice(body).context("Failed to parse planner plans response")?;
    Ok(resp.value.into_iter().map(plan_info).collect())
}

/// Parse a buckets collection body.
pub fn parse_buckets(body: &[u8]) -> Result<Vec<BucketInfo>> {
    let resp: BucketsResponse =
        serde_json::from_slice(body).context("Failed to parse planner buckets response")?;
    Ok(resp.value.into_iter().map(bucket_info).collect())
}

/// Parse a tasks collection body.
pub fn parse_tasks(body: &[u8]) -> Result<Vec<PlannerTaskInfo>> {
    let resp: TasksResponse =
        serde_json::from_slice(body).context("Failed to parse planner tasks response")?;
    Ok(resp.value.into_iter().map(task_info).collect())
}

/// Parse one task body (create/update responses).
pub fn parse_task(body: &[u8]) -> Result<PlannerTaskInfo> {
    let task: PlannerTask =
        serde_json::from_slice(body).context("Failed to parse planner task response")?;
    Ok(task_info(task))
}

/// Fetch a team's plans and return structured data.
pub async fn list_plans_data(client: &TeamsClient, group_id: &str) -> Result<Vec<PlanInfo>> {
    let path = plans_path(group_id)?;
    let resp = client.graph_get(&path).await?;
    let body = resp.bytes().await.context("Failed to read plans body")?;
    parse_plans(&body)
}

/// Fetch one plan's buckets and return structured data.
pub async fn list_buckets_data(client: &TeamsClient, plan_id: &str) -> Result<Vec<BucketInfo>> {
    let path = buckets_path(plan_id)?;
    let resp = client.graph_get(&path).await?;
    let body = resp.bytes().await.context("Failed to read buckets body")?;
    parse_buckets(&body)
}

/// Fetch one plan's tasks and return structured data.
pub async fn list_tasks_data(
    client: &TeamsClient,
    plan_id: &str,
    limit: usize,
) -> Result<Vec<PlannerTaskInfo>> {
    let path = tasks_path(plan_id, limit)?;
    let resp = client.graph_get(&path).await?;
    let body = resp.bytes().await.context("Failed to read tasks body")?;
    parse_tasks(&body)
}

/// Create one task in a bucket and return it. Empty titles are rejected
/// before any network.
pub async fn create_task_data(
    client: &TeamsClient,
    plan_id: &str,
    bucket_id: &str,
    title: &str,
) -> Result<PlannerTaskInfo> {
    let body = create_task_body(plan_id, bucket_id, title)?;
    let resp = client.graph_post("/planner/tasks", &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created task body")?;
    parse_task(&bytes)
}

/// Complete (`complete=true`, 100) or reopen (`false`, 0) one task and
/// return it. Needs the task's current etag for `If-Match`; a 412 means
/// the board moved under the caller (surfaced as the HTTP error detail).
pub async fn set_task_complete_data(
    client: &TeamsClient,
    task_id: &str,
    etag: &str,
    complete: bool,
) -> Result<PlannerTaskInfo> {
    patch_task(client, task_id, etag, &set_complete_body(complete)).await
}

/// Assign (`assign=true`) or unassign one user on one task and return
/// the task. Same `If-Match` / refresh-first contract as
/// [`set_task_complete_data`].
pub async fn set_task_assignee_data(
    client: &TeamsClient,
    task_id: &str,
    etag: &str,
    user_id: &str,
    assign: bool,
) -> Result<PlannerTaskInfo> {
    let body = set_assignment_body(user_id, assign)?;
    patch_task(client, task_id, etag, &body).await
}

/// `PATCH /planner/tasks/{id}` with `If-Match` (via
/// [`TeamsClient::graph_patch_etag`]); a 204 re-reads the task.
async fn patch_task(
    client: &TeamsClient,
    task_id: &str,
    etag: &str,
    body: &serde_json::Value,
) -> Result<PlannerTaskInfo> {
    let path = task_path(task_id)?;
    check_etag(etag)?;
    let resp = client.graph_patch_etag(&path, etag, body).await?;
    let bytes = resp
        .bytes()
        .await
        .context("Failed to read updated task body")?;
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        // 204 No Content: the PATCH applied; read the task back.
        let resp = client.graph_get(&path).await?;
        let bytes = resp
            .bytes()
            .await
            .context("Failed to re-read updated task")?;
        return parse_task(&bytes);
    }
    parse_task(&bytes)
}

/// Raw `PATCH` with `If-Match` and `Prefer: return=representation`
/// (via [`TeamsClient::graph_patch_etag`]). Plans (plan etag, GET
/// first) and details (details etag) go through here; tasks use
/// [`patch_task`].
async fn patch_raw(
    client: &TeamsClient,
    path: &str,
    etag: &str,
    body: &serde_json::Value,
    what: &str,
) -> Result<Vec<u8>> {
    check_etag(etag)?;
    let resp = client.graph_patch_etag(path, etag, body).await?;
    resp.bytes()
        .await
        .with_context(|| format!("Failed to read {} body", what))
        .map(|b| b.to_vec())
}

// ---------------------------------------------------------------------------
// Planner depth (plannerdepth lane): labels, details (checklist /
// description / references), comments (group thread), delete task,
// create/delete plan, create bucket.
//
// Docs grounding (Microsoft Graph v1.0):
// - task labels: `PATCH /planner/tasks/{id}` `{"appliedCategories":
//   {"categoryN": bool}}` (N = 1..=6) with `If-Match`
//   (https://learn.microsoft.com/en-us/graph/api/plannertask-update).
// - label names: `PATCH /planner/plans/{id}` `{"categoryDescriptions":
//   {"categoryN": "name"}}` — no `If-Match`
//   (https://learn.microsoft.com/en-us/graph/api/plannerplan-update).
// - details: `GET/PATCH /planner/tasks/{id}/details` with `If-Match`
//   (details etag): `description`, `checklist` (client-generated GUID
//   keys; null removes), `references` (URL keys; null removes),
//   `previewType`
//   (https://learn.microsoft.com/en-us/graph/api/plannertaskdetails-update).
// - delete task: `DELETE /planner/tasks/{id}` with `If-Match`.
// - create plan: `POST /planner/plans` (`owner` = group id, `title`);
//   delete plan: `DELETE /planner/plans/{id}`.
// - create bucket: `POST /planner/buckets` (`name`, `planId`).
// - comments: Planner surfaces the plan group's conversation thread
//   (`conversationThreadId` on the task); reply with
//   `POST /groups/{id}/threads/{threadId}/reply`, read with
//   `GET /groups/{id}/threads/{threadId}/posts`.

/// `PATCH` path for one plan.
pub fn plan_path(plan_id: &str) -> Result<String> {
    check_id("plan_id", plan_id)?;
    Ok(format!("/planner/plans/{}", plan_id))
}

/// `GET`/`PATCH` path for one task's details.
pub fn task_details_path(task_id: &str) -> Result<String> {
    check_id("task_id", task_id)?;
    Ok(format!("/planner/tasks/{}/details", task_id))
}

/// `POST` path for creating a plan.
pub fn plans_collection_path() -> String {
    "/planner/plans".to_string()
}

/// `POST` path for creating a bucket.
pub fn buckets_collection_path() -> String {
    "/planner/buckets".to_string()
}

/// `POST` path for replying to a plan's group conversation thread.
pub fn thread_reply_path(group_id: &str, thread_id: &str) -> Result<String> {
    check_id("group_id", group_id)?;
    check_id("thread_id", thread_id)?;
    Ok(format!("/groups/{}/threads/{}/reply", group_id, thread_id))
}

/// `GET` path for a group conversation thread's posts.
pub fn thread_posts_path(group_id: &str, thread_id: &str) -> Result<String> {
    check_id("group_id", group_id)?;
    check_id("thread_id", thread_id)?;
    Ok(format!("/groups/{}/threads/{}/posts", group_id, thread_id))
}

/// Label update body: `category` is 1..=6 (`categoryN`), `on` sets or
/// clears it (docs: `appliedCategories`).
pub fn task_labels_body(category: u8, on: bool) -> Result<serde_json::Value> {
    if !(1..=6).contains(&category) {
        bail!("category must be 1-6");
    }
    Ok(serde_json::json!({ "appliedCategories": { format!("category{}", category): on } }))
}

/// Label-names body: `{"categoryDescriptions": {"categoryN": name}}`.
/// `category` is 1..=6; empty names are rejected (omit the entry to
/// leave a name unchanged — Graph has no per-key delete here).
pub fn plan_categories_body(names: &[(u8, String)]) -> Result<serde_json::Value> {
    if names.is_empty() {
        bail!("no categories");
    }
    let mut map = serde_json::Map::new();
    for (n, name) in names {
        if !(1..=6).contains(n) {
            bail!("category must be 1-6");
        }
        if name.trim().is_empty() {
            bail!("empty category name");
        }
        map.insert(format!("category{}", n), serde_json::Value::String(name.clone()));
    }
    Ok(serde_json::json!({ "categoryDescriptions": map }))
}

/// Create-plan body. `owner` is the backing group id (== team id).
pub fn plan_create_body(group_id: &str, title: &str) -> Result<serde_json::Value> {
    check_id("group_id", group_id)?;
    if title.trim().is_empty() {
        bail!("empty title");
    }
    Ok(serde_json::json!({ "owner": group_id, "title": title }))
}

/// Create-bucket body.
pub fn bucket_create_body(plan_id: &str, name: &str) -> Result<serde_json::Value> {
    check_id("plan_id", plan_id)?;
    if name.trim().is_empty() {
        bail!("empty name");
    }
    Ok(serde_json::json!({
        "name": name,
        "planId": plan_id,
        "orderHint": " !",
    }))
}

/// Description body for `PATCH .../details`.
pub fn description_body(description: &str) -> serde_json::Value {
    serde_json::json!({ "description": description })
}

/// One checklist edit: `id` empty = new item (a GUID key is generated
/// and `orderHint " !"` is sent); `checked` sets `isChecked`.
/// `remove=true` ignores title/checked and nulls the key.
pub fn checklist_body(id: &str, title: &str, checked: bool, remove: bool) -> Result<serde_json::Value> {
    let mut map = serde_json::Map::new();
    if remove {
        check_id("item_id", id)?;
        map.insert(id.to_string(), serde_json::Value::Null);
    } else {
        if title.trim().is_empty() {
            bail!("empty title");
        }
        let key = if id.trim().is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            check_id("item_id", id)?;
            id.to_string()
        };
        let mut item = serde_json::Map::new();
        item.insert(
            "@odata.type".to_string(),
            serde_json::Value::String("#microsoft.graph.plannerChecklistItem".to_string()),
        );
        item.insert(
            "title".to_string(),
            serde_json::Value::String(title.to_string()),
        );
        item.insert("isChecked".to_string(), serde_json::Value::Bool(checked));
        if id.trim().is_empty() {
            item.insert(
                "orderHint".to_string(),
                serde_json::Value::String(" !".to_string()),
            );
        }
        map.insert(key, serde_json::Value::Object(item));
    }
    Ok(serde_json::json!({ "checklist": map }))
}

/// Encode a reference URL the way the details endpoint reads it back
/// (docs: `http%3A//developer%2Emicrosoft%2Ecom` — `:` and `.`
/// percent-encoded, `/` left alone). Read-back keys arrive decoded.
pub fn reference_key(url: &str) -> String {
    url.replace(':', "%3A").replace('.', "%2E")
}

/// One reference edit: `attach=true` adds/updates the URL key,
/// `false` nulls it (removes). URLs legitimately contain `/`, `?`,
/// `#`, so only emptiness/whitespace is rejected. The key is
/// [`reference_key`]-encoded (docs; a raw URL key 400s).
pub fn reference_body(url: &str, alias: &str, attach: bool) -> Result<serde_json::Value> {
    if url.trim().is_empty() || url.chars().any(|c| c.is_whitespace()) {
        bail!("bad url");
    }
    let mut map = serde_json::Map::new();
    let key = reference_key(url);
    if attach {
        let mut item = serde_json::Map::new();
        item.insert(
            "@odata.type".to_string(),
            serde_json::Value::String("#microsoft.graph.plannerExternalReference".to_string()),
        );
        item.insert("alias".to_string(), serde_json::Value::String(alias.to_string()));
        map.insert(key, serde_json::Value::Object(item));
    } else {
        map.insert(key, serde_json::Value::Null);
    }
    Ok(serde_json::json!({ "references": map }))
}

/// Comment (group thread reply) body. Plain text; the thread renders it.
pub fn comment_body(text: &str) -> Result<serde_json::Value> {
    if text.trim().is_empty() {
        bail!("empty text");
    }
    Ok(serde_json::json!({ "body": text }))
}

/// One checklist item on task details.
pub struct PlannerChecklistItem {
    pub id: String,
    pub title: String,
    pub checked: bool,
    pub order_hint: Option<String>,
}

/// One external reference on task details.
pub struct PlannerReference {
    pub url: String,
    pub alias: String,
}

/// Task details (`GET/PATCH .../details`).
pub struct PlannerTaskDetails {
    pub etag: String,
    pub description: String,
    pub checklist: Vec<PlannerChecklistItem>,
    pub references: Vec<PlannerReference>,
    pub preview_type: Option<String>,
}

/// One group conversation post (a Planner comment).
pub struct GroupPost {
    pub id: String,
    pub content_type: Option<String>,
    pub content: String,
    pub received: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskDetailsResponse {
    description: Option<String>,
    checklist: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    references: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    #[serde(rename = "previewType")]
    preview_type: Option<String>,
    #[serde(rename = "@odata.etag")]
    etag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadPostsResponse {
    value: Vec<ThreadPost>,
}

#[derive(Debug, Deserialize)]
struct ThreadPost {
    id: String,
    body: Option<ThreadPostBody>,
    #[serde(rename = "receivedDateTime")]
    received: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadPostBody {
    #[serde(rename = "contentType")]
    content_type: Option<String>,
    content: Option<String>,
}

/// Decode a reference key read back from details (`:` and `.` arrive
/// percent-encoded; docs write them back encoded too).
pub fn reference_decode(key: &str) -> String {
    key.replace("%3A", ":").replace("%3a", ":").replace("%2E", ".").replace("%2e", ".")
}

fn details_info(d: TaskDetailsResponse) -> PlannerTaskDetails {
    let mut checklist: Vec<PlannerChecklistItem> = d
        .checklist
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, v)| {
            let title = v.get("title")?.as_str()?.to_string();
            if title.trim().is_empty() {
                return None;
            }
            Some(PlannerChecklistItem {
                id,
                title,
                checked: v.get("isChecked")?.as_bool().unwrap_or(false),
                order_hint: v
                    .get("orderHint")
                    .and_then(|h| h.as_str())
                    .map(str::to_string),
            })
        })
        .collect();
    checklist.sort_by(|a, b| a.order_hint.cmp(&b.order_hint).then(a.id.cmp(&b.id)));
    let mut references: Vec<PlannerReference> = d
        .references
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(url, v)| {
            if url.trim().is_empty() || v.is_null() {
                return None;
            }
            Some(PlannerReference {
                url: reference_decode(&url),
                alias: v
                    .get("alias")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect();
    references.sort_by(|a, b| a.url.cmp(&b.url));
    PlannerTaskDetails {
        etag: d.etag.unwrap_or_default(),
        description: d.description.unwrap_or_default(),
        checklist,
        references,
        preview_type: d.preview_type,
    }
}

/// Parse a task-details body (pure; powers tests + embedders).
pub fn parse_task_details(body: &[u8]) -> Result<PlannerTaskDetails> {
    let resp: TaskDetailsResponse =
        serde_json::from_slice(body).context("Failed to parse planner task details response")?;
    Ok(details_info(resp))
}

/// Parse a thread-posts collection body.
pub fn parse_thread_posts(body: &[u8]) -> Result<Vec<GroupPost>> {
    let resp: ThreadPostsResponse =
        serde_json::from_slice(body).context("Failed to parse thread posts response")?;
    Ok(resp
        .value
        .into_iter()
        .map(|p| GroupPost {
            id: p.id,
            content_type: p.body.as_ref().and_then(|b| b.content_type.clone()),
            content: p.body.as_ref().and_then(|b| b.content.clone()).unwrap_or_default(),
            received: p.received,
        })
        .collect())
}

/// Set (`on=true`) or clear one label (1..=6) on a task and return it.
pub async fn set_task_labels_data(
    client: &TeamsClient,
    task_id: &str,
    etag: &str,
    category: u8,
    on: bool,
) -> Result<PlannerTaskInfo> {
    let body = task_labels_body(category, on)?;
    patch_task(client, task_id, etag, &body).await
}

/// Rename plan labels (`categoryDescriptions`) and return the plan.
/// Live-proved: plan PATCH requires `If-Match` (412 without), so the
/// plan is GET first for its etag.
pub async fn set_plan_categories_data(
    client: &TeamsClient,
    plan_id: &str,
    names: &[(u8, String)],
) -> Result<PlanInfo> {
    let body = plan_categories_body(names)?;
    let plan = get_plan_data(client, plan_id).await?;
    anyhow::ensure!(!plan.etag.trim().is_empty(), "plan has no etag for If-Match");
    let path = plan_path(plan_id)?;
    patch_raw(client, &path, &plan.etag, &body, "updated plan").await?;
    // The PATCH representation does not echo categoryDescriptions
    // (live-proved); the GET below is the read-back.
    get_plan_data(client, plan_id).await
}

/// Parse one plan body (create/update responses).
pub fn parse_plan(body: &[u8]) -> Result<PlanInfo> {
    let plan: Plan =
        serde_json::from_slice(body).context("Failed to parse planner plan response")?;
    Ok(plan_info(plan))
}

/// Parse one bucket body (create response).
pub fn parse_bucket(body: &[u8]) -> Result<BucketInfo> {
    let bucket: Bucket =
        serde_json::from_slice(body).context("Failed to parse planner bucket response")?;
    Ok(bucket_info(bucket))
}

/// Create one plan under a group and return it.
pub async fn create_plan_data(
    client: &TeamsClient,
    group_id: &str,
    title: &str,
) -> Result<PlanInfo> {
    let body = plan_create_body(group_id, title)?;
    let resp = client.graph_post(&plans_collection_path(), &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created plan body")?;
    parse_plan(&bytes)
}

/// Create one bucket in a plan and return it.
pub async fn create_bucket_data(
    client: &TeamsClient,
    plan_id: &str,
    name: &str,
) -> Result<BucketInfo> {
    let body = bucket_create_body(plan_id, name)?;
    let resp = client.graph_post(&buckets_collection_path(), &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created bucket body")?;
    parse_bucket(&bytes)
}

/// Fetch one task's details.
pub async fn get_task_details_data(
    client: &TeamsClient,
    task_id: &str,
) -> Result<PlannerTaskDetails> {
    let path = task_details_path(task_id)?;
    let resp = client.graph_get(&path).await?;
    let body = resp.bytes().await.context("Failed to read task details body")?;
    parse_task_details(&body)
}

/// `PATCH` one task's details with a prebuilt body; returns the details
/// etag for chaining (details PATCH answers 204, no body).
pub async fn patch_task_details_data(
    client: &TeamsClient,
    task_id: &str,
    etag: &str,
    body: &serde_json::Value,
) -> Result<()> {
    let path = task_details_path(task_id)?;
    patch_raw(client, &path, etag, body, "updated task details").await?;
    Ok(())
}

/// Set a task's description (details round-trip inside).
pub async fn set_task_description_data(
    client: &TeamsClient,
    task_id: &str,
    description: &str,
) -> Result<()> {
    let details = get_task_details_data(client, task_id).await?;
    patch_task_details_data(client, task_id, &details.etag, &description_body(description)).await
}

/// Add/update (`remove=false`) or remove one checklist item.
pub async fn set_task_checklist_data(
    client: &TeamsClient,
    task_id: &str,
    item_id: &str,
    title: &str,
    checked: bool,
    remove: bool,
) -> Result<()> {
    let details = get_task_details_data(client, task_id).await?;
    let body = checklist_body(item_id, title, checked, remove)?;
    patch_task_details_data(client, task_id, &details.etag, &body).await
}

/// Attach (`attach=true`) or detach one URL reference.
pub async fn set_task_reference_data(
    client: &TeamsClient,
    task_id: &str,
    url: &str,
    alias: &str,
    attach: bool,
) -> Result<()> {
    let details = get_task_details_data(client, task_id).await?;
    let body = reference_body(url, alias, attach)?;
    patch_task_details_data(client, task_id, &details.etag, &body).await
}

/// Raw `DELETE` with `If-Match` (via
/// [`TeamsClient::graph_delete_etag`]); tasks and plans require one.
async fn delete_raw(client: &TeamsClient, path: &str, etag: &str) -> Result<()> {
    check_etag(etag)?;
    client.graph_delete_etag(path, Some(etag)).await?;
    Ok(())
}

/// Delete one task (`If-Match` with the task etag; answers 204).
pub async fn delete_task_data(
    client: &TeamsClient,
    task_id: &str,
    etag: &str,
) -> Result<()> {
    let path = task_path(task_id)?;
    delete_raw(client, &path, etag).await
}

/// Fetch one plan and return it.
pub async fn get_plan_data(client: &TeamsClient, plan_id: &str) -> Result<PlanInfo> {
    let path = plan_path(plan_id)?;
    let resp = client.graph_get(&path).await?;
    let body = resp.bytes().await.context("Failed to read plan body")?;
    parse_plan(&body)
}

/// Delete one plan (answers 204). Live-proved: plan DELETE requires
/// `If-Match` (412 without), so the plan is GET first for its etag.
pub async fn delete_plan_data(client: &TeamsClient, plan_id: &str) -> Result<()> {
    let plan = get_plan_data(client, plan_id).await?;
    anyhow::ensure!(!plan.etag.trim().is_empty(), "plan has no etag for If-Match");
    let path = plan_path(plan_id)?;
    delete_raw(client, &path, &plan.etag).await
}

/// `POST` path for creating a group conversation thread.
pub fn threads_collection_path(group_id: &str) -> Result<String> {
    check_id("group_id", group_id)?;
    Ok(format!("/groups/{}/threads", group_id))
}

/// `DELETE` path for one group conversation thread.
pub fn thread_path(group_id: &str, thread_id: &str) -> Result<String> {
    check_id("group_id", group_id)?;
    check_id("thread_id", thread_id)?;
    Ok(format!("/groups/{}/threads/{}", group_id, thread_id))
}

/// Thread-create body: one opening text post (docs: `POST
/// /groups/{id}/threads` with `posts`).
pub fn thread_create_body(topic: &str) -> Result<serde_json::Value> {
    if topic.trim().is_empty() {
        bail!("empty topic");
    }
    Ok(serde_json::json!({ "posts": [{ "body": {
        "contentType": "Text", "content": topic } }] }))
}

/// Create one group conversation thread (the probe's comment home) and
/// return its id. Answers 201 with the thread.
pub async fn create_thread_data(client: &TeamsClient, group_id: &str, topic: &str) -> Result<String> {
    let body = thread_create_body(topic)?;
    let path = threads_collection_path(group_id)?;
    let resp = client.graph_post(&path, &body).await?;
    let bytes = resp.bytes().await.context("Failed to read created thread body")?;
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).context("Failed to parse created thread response")?;
    v.get("id")
        .and_then(|id| id.as_str())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .context("Created thread response has no id")
}

/// Delete one group conversation thread (probe cleanup; answers 204).
pub async fn delete_thread_data(
    client: &TeamsClient,
    group_id: &str,
    thread_id: &str,
) -> Result<()> {
    let path = thread_path(group_id, thread_id)?;
    client.graph_delete_etag(&path, None).await?;
    Ok(())
}

/// Post one Planner comment: a reply to the task's group thread.
pub async fn post_task_comment_data(
    client: &TeamsClient,
    group_id: &str,
    thread_id: &str,
    text: &str,
) -> Result<()> {
    let body = comment_body(text)?;
    let path = thread_reply_path(group_id, thread_id)?;
    client.graph_post(&path, &body).await?;
    Ok(())
}

/// Read a plan thread's posts (Planner comments, oldest first as sent).
pub async fn list_thread_posts_data(
    client: &TeamsClient,
    group_id: &str,
    thread_id: &str,
) -> Result<Vec<GroupPost>> {
    let path = thread_posts_path(group_id, thread_id)?;
    let resp = client.graph_get(&path).await?;
    let body = resp.bytes().await.context("Failed to read thread posts body")?;
    parse_thread_posts(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_guard_rejects_path_breaking() {
        for bad in ["", "   ", "a/b", "a?b", "a#b", "a b", "a\tb"] {
            assert!(check_id("plan_id", bad).is_err(), "id {:?}", bad);
            assert!(plans_path(bad).is_err());
            assert!(buckets_path(bad).is_err());
            assert!(tasks_path(bad, 50).is_err());
            assert!(task_path(bad).is_err());
        }
        assert!(check_id("plan_id", "Ckkc-9r0N0WsmxebQybwRGUACo8z").is_ok());
    }

    #[test]
    fn etag_guard_rejects_empty_and_crlf() {
        assert!(check_etag("").is_err());
        assert!(check_etag("   ").is_err());
        assert!(check_etag("W/\"abc\"\r\nX: y").is_err());
        assert!(check_etag("W/\"JzI4VGVzMUJxdF9HdERxOFB3QTRBQT09\"").is_ok());
    }

    #[test]
    fn paths_match_graph_docs() {
        assert_eq!(
            plans_path("g1").unwrap(),
            "/groups/g1/planner/plans"
        );
        assert_eq!(buckets_path("p1").unwrap(), "/planner/plans/p1/buckets");
        assert_eq!(
            tasks_path("p1", 50).unwrap(),
            "/planner/plans/p1/tasks?$top=50"
        );
        assert_eq!(task_path("t1").unwrap(), "/planner/tasks/t1");
    }

    #[test]
    fn bodies_match_graph_docs() {
        assert_eq!(
            set_complete_body(true),
            serde_json::json!({ "percentComplete": 100 })
        );
        assert_eq!(
            set_complete_body(false),
            serde_json::json!({ "percentComplete": 0 })
        );
        assert_eq!(
            create_task_body("p1", "b1", "Do it").unwrap(),
            serde_json::json!({ "planId": "p1", "bucketId": "b1", "title": "Do it" })
        );
        assert!(create_task_body("p1", "b1", "  ").is_err());
        assert!(create_task_body("p/", "b1", "x").is_err());
        assert!(create_task_body("p1", "b/", "x").is_err());
    }

    #[test]
    fn plans_parse_with_title_fallback() {
        let infos = parse_plans(
            br#"{"value":[
                {"id":"P1","title":"Sprint 12"},
                {"id":"P2","title":""},
                {"id":"P3"}]}"#,
        )
        .unwrap();
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].title, "Sprint 12");
        assert_eq!(infos[1].title, "P2");
        assert_eq!(infos[2].title, "P3");
    }

    #[test]
    fn buckets_parse_with_name_fallback() {
        let infos = parse_buckets(
            br#"{"value":[
                {"id":"B1","name":"To do","planId":"P1"},
                {"id":"B2"}]}"#,
        )
        .unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "To do");
        assert_eq!(infos[0].plan_id, "P1");
        assert_eq!(infos[1].name, "B2");
        assert_eq!(infos[1].plan_id, "");
    }

    #[test]
    fn tasks_parse_percent_priority_due_etag() {
        let infos = parse_tasks(
            br#"{"value":[
                {"id":"T1","planId":"P1","bucketId":"B1","title":"Ship it",
                 "percentComplete":50,"priority":1,
                 "dueDateTime":"2026-10-02T12:00:00Z",
                 "@odata.etag":"W/\"etag-1\""},
                {"id":"T2","title":"Done","percentComplete":100,
                 "@odata.etag":"W/\"etag-2\""},
                {"id":"T3"}]}"#,
        )
        .unwrap();
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].title, "Ship it");
        assert_eq!(infos[0].percent_complete, 50);
        assert!(!infos[0].completed);
        assert_eq!(infos[0].priority, Some(1));
        assert_eq!(infos[0].due.as_deref(), Some("2026-10-02T12:00:00Z"));
        assert_eq!(infos[0].etag, "W/\"etag-1\"");
        assert_eq!(infos[0].bucket_id, "B1");
        assert!(infos[1].completed);
        assert_eq!(infos[1].priority, None);
        assert!(infos[1].due.is_none());
        assert_eq!(infos[2].percent_complete, 0); // missing -> 0, never panic
        assert_eq!(infos[2].title, "");
        assert_eq!(infos[2].etag, "");
        assert!(!infos[2].completed);
    }

    #[test]
    fn single_task_parses() {
        let info = parse_task(
            br#"{"id":"T9","planId":"P1","bucketId":"B1","title":"New",
                 "percentComplete":0,"@odata.etag":"W/\"etag-9\""}"#,
        )
        .unwrap();
        assert_eq!(info.id, "T9");
        assert!(!info.completed);
        assert_eq!(info.etag, "W/\"etag-9\"");
    }

    #[test]
    fn assignments_parse_and_bodies_match_graph_docs() {
        let info = parse_task(
            br##"{"id":"T1","title":"x","@odata.etag":"W/\"e\"",
                 "assignments":{
                   "u-b":{"@odata.type":"#microsoft.graph.plannerAssignment","orderHint":" !"},
                   "u-a":{"orderHint":"8585"},
                   "u-gone":null}}"##,
        )
        .unwrap();
        assert_eq!(info.assignee_ids, vec!["u-a", "u-b"]);
        let none = parse_task(br#"{"id":"T2"}"#).unwrap();
        assert!(none.assignee_ids.is_empty());
        assert_eq!(
            set_assignment_body("u1", true).unwrap(),
            serde_json::json!({ "assignments": { "u1": {
                "@odata.type": "#microsoft.graph.plannerAssignment",
                "orderHint": " !" } } })
        );
        assert_eq!(
            set_assignment_body("u1", false).unwrap(),
            serde_json::json!({ "assignments": { "u1": null } })
        );
        assert!(set_assignment_body("u/1", true).is_err());
        assert!(set_assignment_body(" ", false).is_err());
    }

    #[test]
    fn garbage_bodies_error() {
        assert!(parse_plans(b"nope").is_err());
        assert!(parse_buckets(b"{}").is_err());
        assert!(parse_tasks(b"[]").is_err());
        assert!(parse_task(b"{}").is_err()); // id required
    }

    #[test]
    fn thread_paths_and_body_match_graph_docs() {
        assert_eq!(
            threads_collection_path("g1").unwrap(),
            "/groups/g1/threads"
        );
        assert_eq!(
            thread_path("g1", "th1").unwrap(),
            "/groups/g1/threads/th1"
        );
        assert!(threads_collection_path("g/1").is_err());
        assert!(thread_path("g1", "t h").is_err());
        assert_eq!(
            thread_create_body("hello").unwrap(),
            serde_json::json!({ "posts": [{ "body": {
                "contentType": "Text", "content": "hello" } }] })
        );
        assert!(thread_create_body("  ").is_err());
    }

    #[test]
    fn depth_paths_match_graph_docs() {
        assert_eq!(plan_path("p1").unwrap(), "/planner/plans/p1");
        assert_eq!(
            task_details_path("t1").unwrap(),
            "/planner/tasks/t1/details"
        );
        assert_eq!(plans_collection_path(), "/planner/plans");
        assert_eq!(buckets_collection_path(), "/planner/buckets");
        assert_eq!(
            thread_reply_path("g1", "th1").unwrap(),
            "/groups/g1/threads/th1/reply"
        );
        assert_eq!(
            thread_posts_path("g1", "th1").unwrap(),
            "/groups/g1/threads/th1/posts"
        );
        assert!(plan_path("p/1").is_err());
        assert!(task_details_path(" ").is_err());
        assert!(thread_reply_path("g1", "t h").is_err());
        assert!(thread_posts_path("g/1", "th1").is_err());
    }

    #[test]
    fn depth_bodies_match_graph_docs() {
        assert_eq!(
            task_labels_body(1, true).unwrap(),
            serde_json::json!({ "appliedCategories": { "category1": true } })
        );
        assert_eq!(
            task_labels_body(6, false).unwrap(),
            serde_json::json!({ "appliedCategories": { "category6": false } })
        );
        assert!(task_labels_body(0, true).is_err());
        assert!(task_labels_body(7, true).is_err());
        assert_eq!(
            plan_categories_body(&[(1, "Red".to_string()), (3, "Blue".to_string())]).unwrap(),
            serde_json::json!({ "categoryDescriptions": {
                "category1": "Red", "category3": "Blue" } })
        );
        assert!(plan_categories_body(&[]).is_err());
        assert!(plan_categories_body(&[(0, "x".to_string())]).is_err());
        assert!(plan_categories_body(&[(1, "  ".to_string())]).is_err());
        assert_eq!(
            plan_create_body("g1", "Board").unwrap(),
            serde_json::json!({ "owner": "g1", "title": "Board" })
        );
        assert!(plan_create_body("g1", "  ").is_err());
        assert!(plan_create_body("g/1", "Board").is_err());
        assert_eq!(
            bucket_create_body("p1", "To do").unwrap(),
            serde_json::json!({
                "name": "To do", "planId": "p1", "orderHint": " !" })
        );
        assert!(bucket_create_body("p1", " ").is_err());
        assert_eq!(
            description_body("notes here"),
            serde_json::json!({ "description": "notes here" })
        );
        assert_eq!(reference_key("https://example.com/a"), "https%3A//example%2Ecom/a");
        assert_eq!(
            reference_body("https://example.com/a", "Spec", true).unwrap(),
            serde_json::json!({ "references": { "https%3A//example%2Ecom/a": {
                "@odata.type": "#microsoft.graph.plannerExternalReference",
                "alias": "Spec" } } })
        );
        assert_eq!(
            reference_body("https://example.com/a", "", false).unwrap(),
            serde_json::json!({ "references": { "https%3A//example%2Ecom/a": null } })
        );
        assert!(reference_body(" ", "x", true).is_err());
        assert!(reference_body("has space.com/a b", "x", true).is_err());
        assert_eq!(
            comment_body("looks good").unwrap(),
            serde_json::json!({ "body": "looks good" })
        );
        assert!(comment_body("  ").is_err());
    }

    #[test]
    fn checklist_body_new_update_remove() {
        // New item: empty id generates a GUID key with an orderHint.
        let new = checklist_body("", "Buy milk", false, false).unwrap();
        let map = new["checklist"].as_object().unwrap();
        assert_eq!(map.len(), 1);
        let (key, item) = map.iter().next().unwrap();
        assert!(uuid::Uuid::parse_str(key).is_ok(), "key {:?}", key);
        assert_eq!(item["title"], serde_json::json!("Buy milk"));
        assert_eq!(item["isChecked"], serde_json::json!(false));
        assert_eq!(item["orderHint"], serde_json::json!(" !"));
        // Update: existing id round-trips with no orderHint (no reorder).
        let upd = checklist_body("iid-1", "Buy oat milk", true, false).unwrap();
        let item = &upd["checklist"]["iid-1"];
        assert_eq!(item["title"], serde_json::json!("Buy oat milk"));
        assert_eq!(item["isChecked"], serde_json::json!(true));
        assert!(item.get("orderHint").is_none());
        // Remove: null value, title ignored.
        assert_eq!(
            checklist_body("iid-1", "", false, true).unwrap(),
            serde_json::json!({ "checklist": { "iid-1": null } })
        );
        assert!(checklist_body("", "", false, false).is_err());
        assert!(checklist_body("", "x", false, true).is_err());
    }

    #[test]
    fn labels_thread_categories_parse() {
        let info = parse_task(
            br##"{"id":"T1","appliedCategories":{"category2":true,"category1":false,
                 "category9":true,"nope":true},
                 "conversationThreadId":"thread-1"}"##,
        )
        .unwrap();
        assert_eq!(info.applied_labels, vec![2]);
        assert_eq!(info.thread_id, "thread-1");
        let bare = parse_task(br#"{"id":"T2"}"#).unwrap();
        assert!(bare.applied_labels.is_empty());
        assert!(bare.thread_id.is_empty());
        let plan = parse_plan(
            br#"{"id":"P1","title":"Sprint","@odata.etag":"W/\"p-etag\"",
                 "categoryDescriptions":{"category1":"Red","category2":null,
                 "category7":"Out of range","other":"x"}}"#,
        )
        .unwrap();
        assert_eq!(
            plan.categories,
            vec![(1, "Red".to_string())]
        );
        assert_eq!(plan.etag, "W/\"p-etag\"");
        let plain = parse_plan(br#"{"id":"P9"}"#).unwrap();
        assert_eq!(plain.title, "P9");
        assert!(plain.categories.is_empty());
    }

    #[test]
    fn details_and_posts_parse() {
        let d = parse_task_details(
            br#"{"description":"Ship it",
                 "previewType":"checklist",
                 "@odata.etag":"W/\"d-etag\"",
                 "checklist":{
                   "c-b":{"title":"Second","isChecked":true,"orderHint":"8585"},
                   "c-a":{"title":"First","isChecked":false,"orderHint":" !"},
                   "c-x":{"title":"","isChecked":false}},
                 "references":{
                   "https%3A//example%2Ecom/spec":{"alias":"Spec"},
                   "https://example.com/gone":null}}"#,
        )
        .unwrap();
        assert_eq!(d.etag, "W/\"d-etag\"");
        assert_eq!(d.description, "Ship it");
        assert_eq!(d.preview_type.as_deref(), Some("checklist"));
        assert_eq!(d.checklist.len(), 2); // blank title dropped
        assert_eq!(d.checklist[0].id, "c-a"); // orderHint order
        assert!(!d.checklist[0].checked);
        assert!(d.checklist[1].checked);
        assert_eq!(d.references.len(), 1); // null dropped
        assert_eq!(d.references[0].url, "https://example.com/spec"); // key decoded
        assert_eq!(d.references[0].alias, "Spec");
        assert_eq!(reference_decode("https%3A//x%2Ecom/a"), "https://x.com/a");
        let posts = parse_thread_posts(
            br#"{"value":[
                 {"id":"p2","receivedDateTime":"2026-11-19T10:00:00Z",
                  "body":{"contentType":"Text","content":"second"}},
                 {"id":"p1","body":{"contentType":"html","content":"<p>first</p>"}}]}"#,
        )
        .unwrap();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].id, "p2");
        assert_eq!(posts[0].content, "second");
        assert_eq!(posts[1].content, "<p>first</p>");
        assert!(parse_task_details(b"[]").is_err());
    }
}
