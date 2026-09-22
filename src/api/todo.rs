//! Microsoft To Do lists and tasks (Graph `/me/todo`).
//!
//! Reads the signed-in user's real Microsoft To Do data. Auth reuses the
//! existing Graph token (`https://graph.microsoft.com/.default` +
//! `offline_access`); no scope widening: the first-party Teams client id
//! already consents `Tasks.ReadWrite`, and a 403 surfaces as the call's detail.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

#[derive(Debug, Deserialize)]
struct TodoListsResponse {
    value: Vec<TodoList>,
}

#[derive(Debug, Deserialize)]
struct TodoList {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "wellknownListName")]
    wellknown_list_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TodoTasksResponse {
    value: Vec<TodoTask>,
}

#[derive(Debug, Deserialize)]
struct DateTimeTimeZone {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    // Kept: part of the Graph shape (only dateTime is surfaced today).
    #[serde(rename = "timeZone")]
    #[allow(dead_code)]
    time_zone: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TodoTask {
    id: String,
    title: Option<String>,
    status: Option<String>,
    importance: Option<String>,
    #[serde(rename = "dueDateTime")]
    due_date_time: Option<DateTimeTimeZone>,
    #[serde(rename = "reminderDateTime")]
    reminder_date_time: Option<DateTimeTimeZone>,
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

fn zoned(dt: &Option<DateTimeTimeZone>) -> Option<String> {
    dt.as_ref().and_then(|d| d.date_time.clone())
}

/// List To Do lists (prints to stdout).
pub async fn list_todo_lists() -> Result<()> {
    let client = TeamsClient::new().await?;
    let lists = list_todo_lists_data(&client).await?;

    println!("\nTo Do Lists:");
    println!("{:-<60}", "");

    if lists.is_empty() {
        println!("  (no lists found)");
        return Ok(());
    }

    for list in &lists {
        let tag = list
            .wellknown
            .as_deref()
            .map(|w| format!(" [{}]", w))
            .unwrap_or_default();
        println!("{}{}", list.name, tag);
        println!("  ID: {}", list.id);
        println!();
    }

    Ok(())
}

/// List tasks in one To Do list (prints to stdout).
pub async fn list_todo_tasks(list_id: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let tasks = list_todo_tasks_data(&client, list_id, limit).await?;

    println!("\nTasks:");
    println!("{:-<60}", "");

    if tasks.is_empty() {
        println!("  (no tasks)");
        return Ok(());
    }

    for task in &tasks {
        let box_ = if task.completed { "[x]" } else { "[ ]" };
        let due = task
            .due
            .as_deref()
            .map(|d| format!(" (due {})", d))
            .unwrap_or_default();
        println!("{} {}{}", box_, task.title, due);
        println!("  ID: {}", task.id);
        println!();
    }

    Ok(())
}

/// Create one task in a list (prints the new id to stdout).
pub async fn create_todo_task(list_id: &str, title: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let task = create_todo_task_data(&client, list_id, title).await?;
    println!("Created: {} ({})", task.title, task.id);
    Ok(())
}

/// Mark one task completed (prints to stdout).
pub async fn complete_todo_task(list_id: &str, task_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    let task = complete_todo_task_data(&client, list_id, task_id).await?;
    println!("Completed: {} ({})", task.title, task.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Data-returning API functions (no printing; for reuse by other callers)
// ---------------------------------------------------------------------------

/// To Do list metadata.
pub struct TodoListInfo {
    pub id: String,
    pub name: String,
    /// `defaultList` / `flaggedEmails` / `unknownFutureValue`, when set.
    pub wellknown: Option<String>,
}

/// To Do task metadata.
#[allow(dead_code)]
pub struct TodoTaskInfo {
    pub id: String,
    pub title: String,
    /// Raw Graph status: notStarted, inProgress, completed, waitingOnOthers,
    /// deferred (plus future server values, passed through).
    pub status: String,
    /// low, normal, high (plus future server values, passed through).
    pub importance: String,
    /// `dueDateTime.dateTime` verbatim, when set.
    pub due: Option<String>,
    /// `reminderDateTime.dateTime` verbatim, when set.
    pub reminder: Option<String>,
    /// True when `status` is `completed` (case-insensitive).
    pub completed: bool,
}

fn list_info(l: TodoList) -> TodoListInfo {
    let name = l
        .display_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| l.id.clone());
    TodoListInfo {
        id: l.id,
        name,
        wellknown: l.wellknown_list_name,
    }
}

fn task_info(t: TodoTask) -> TodoTaskInfo {
    let status = t.status.unwrap_or_else(|| "notStarted".to_string());
    TodoTaskInfo {
        id: t.id,
        title: t.title.unwrap_or_default(),
        completed: status.eq_ignore_ascii_case("completed"),
        status,
        importance: t.importance.unwrap_or_else(|| "normal".to_string()),
        due: zoned(&t.due_date_time),
        reminder: zoned(&t.reminder_date_time),
    }
}

/// Fetch To Do lists and return structured data.
pub async fn list_todo_lists_data(client: &TeamsClient) -> Result<Vec<TodoListInfo>> {
    let resp = client.graph_get("/me/todo/lists").await?;
    let lists: TodoListsResponse = resp
        .json()
        .await
        .context("Failed to parse todo lists response")?;
    Ok(lists.value.into_iter().map(list_info).collect())
}

/// Fetch tasks for one list and return structured data.
pub async fn list_todo_tasks_data(
    client: &TeamsClient,
    list_id: &str,
    limit: usize,
) -> Result<Vec<TodoTaskInfo>> {
    check_id("list_id", list_id)?;
    let path = format!("/me/todo/lists/{}/tasks?$top={}", list_id, limit);
    let resp = client.graph_get(&path).await?;
    let tasks: TodoTasksResponse = resp
        .json()
        .await
        .context("Failed to parse todo tasks response")?;
    Ok(tasks.value.into_iter().map(task_info).collect())
}

/// Create one task in a list and return it. Empty titles are rejected
/// before any network.
pub async fn create_todo_task_data(
    client: &TeamsClient,
    list_id: &str,
    title: &str,
) -> Result<TodoTaskInfo> {
    check_id("list_id", list_id)?;
    if title.trim().is_empty() {
        bail!("empty title");
    }
    let path = format!("/me/todo/lists/{}/tasks", list_id);
    let body = serde_json::json!({ "title": title });
    let resp = client.graph_post(&path, &body).await?;
    let task: TodoTask = resp
        .json()
        .await
        .context("Failed to parse created todo task response")?;
    Ok(task_info(task))
}

/// Mark one task completed and return it.
pub async fn complete_todo_task_data(
    client: &TeamsClient,
    list_id: &str,
    task_id: &str,
) -> Result<TodoTaskInfo> {
    check_id("list_id", list_id)?;
    check_id("task_id", task_id)?;
    let path = format!("/me/todo/lists/{}/tasks/{}", list_id, task_id);
    let body = serde_json::json!({ "status": "completed" });
    let resp = client.graph_patch(&path, &body).await?;
    let task: TodoTask = resp
        .json()
        .await
        .context("Failed to parse completed todo task response")?;
    Ok(task_info(task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_guard_rejects_path_breaking() {
        for bad in ["", "   ", "a/b", "a?b", "a#b", "a b", "a\tb"] {
            assert!(check_id("list_id", bad).is_err(), "id {:?}", bad);
        }
        assert!(check_id("list_id", "AAMkAGY-123_=").is_ok());
    }

    #[test]
    fn lists_parse_with_wellknown_and_fallback() {
        let body: TodoListsResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"L1","displayName":"Tasks","wellknownListName":"defaultList"},
                {"id":"L2","displayName":"","wellknownListName":null},
                {"id":"L3"}]}"#,
        )
        .unwrap();
        let infos: Vec<_> = body.value.into_iter().map(list_info).collect();
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].name, "Tasks");
        assert_eq!(infos[0].wellknown.as_deref(), Some("defaultList"));
        assert_eq!(infos[1].name, "L2"); // empty displayName falls back to id
        assert!(infos[1].wellknown.is_none());
        assert_eq!(infos[2].name, "L3"); // missing displayName falls back to id
    }

    #[test]
    fn tasks_parse_status_dates_importance() {
        let body: TodoTasksResponse = serde_json::from_str(
            r#"{"value":[
                {"id":"T1","title":"Buy milk","status":"notStarted","importance":"high",
                 "dueDateTime":{"dateTime":"2026-09-23T12:00:00.0000000","timeZone":"UTC"},
                 "reminderDateTime":{"dateTime":"2026-09-23T11:00:00.0000000","timeZone":"UTC"}},
                {"id":"T2","title":"Done thing","status":"Completed","importance":"low"},
                {"id":"T3"}]}"#,
        )
        .unwrap();
        let infos: Vec<_> = body.value.into_iter().map(task_info).collect();
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].title, "Buy milk");
        assert!(!infos[0].completed);
        assert_eq!(infos[0].importance, "high");
        assert_eq!(
            infos[0].due.as_deref(),
            Some("2026-09-23T12:00:00.0000000")
        );
        assert_eq!(
            infos[0].reminder.as_deref(),
            Some("2026-09-23T11:00:00.0000000")
        );
        assert!(infos[1].completed); // case-insensitive
        assert!(infos[1].due.is_none());
        assert_eq!(infos[2].status, "notStarted"); // default
        assert_eq!(infos[2].importance, "normal"); // default
        assert_eq!(infos[2].title, ""); // missing title -> empty, never panic
        assert!(!infos[2].completed);
    }
}
