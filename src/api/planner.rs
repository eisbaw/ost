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
//!
//! The `If-Match` PATCH goes through [`TeamsClient::graph_patch_etag`],
//! which also sends `Prefer: return=representation` (docs: 200 + the
//! updated task instead of 204-empty). When the server still answers
//! 204, [`set_task_complete_data`] re-fetches the task so the caller
//! always gets the updated record.

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
    /// `@odata.etag` verbatim; required for complete/reopen.
    pub etag: String,
}

fn plan_info(p: Plan) -> PlanInfo {
    let title = p
        .title
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| p.id.clone());
    PlanInfo { id: p.id, title }
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
    }
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
    let path = task_path(task_id)?;
    check_etag(etag)?;
    let body = set_complete_body(complete);
    let resp = client.graph_patch_etag(&path, etag, &body).await?;
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
    fn garbage_bodies_error() {
        assert!(parse_plans(b"nope").is_err());
        assert!(parse_buckets(b"{}").is_err());
        assert!(parse_tasks(b"[]").is_err());
        assert!(parse_task(b"{}").is_err()); // id required
    }
}
