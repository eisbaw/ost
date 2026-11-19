//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod graph;
mod me;
mod planner;
mod presence;
mod teams;

use anyhow::Result;

// Re-export data types for TUI integration
pub use chat::{ChatInfo, MessageInfo};
pub use me::UserInfo;
pub use planner::{BucketInfo, GroupPost, PlanInfo, PlannerChecklistItem, PlannerReference, PlannerTaskDetails, PlannerTaskInfo};
pub use presence::PresenceInfo;
pub use teams::TeamInfo;

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use planner::{
    bucket_create_body, buckets_collection_path, buckets_path,
    checklist_body, comment_body, create_bucket_data, create_plan_data, create_task_body,
    create_task_data, create_thread_data, delete_plan_data, delete_task_data, delete_thread_data,
    description_body, get_plan_data, get_task_details_data, list_buckets_data, list_plans_data,
    list_tasks_data, list_thread_posts_data, parse_bucket, parse_buckets, parse_plan, parse_plans,
    parse_task, parse_task_details, parse_tasks, parse_thread_posts, patch_task_details_data,
    plan_categories_body, plan_create_body, plan_path, plans_collection_path, post_task_comment_data,
    reference_body, reference_decode, reference_key,
    set_assignment_body, set_complete_body, set_plan_categories_data, set_task_assignee_data,
    set_task_checklist_data, set_task_complete_data, set_task_description_data, set_task_labels_data,
    set_task_reference_data, task_details_path, task_labels_body, task_path, tasks_path,
    thread_create_body, thread_path, thread_posts_path, thread_reply_path, threads_collection_path,
};
pub use presence::get_presence_data;
pub use teams::list_teams_data;

/// List recent chats (native Teams API)
pub async fn list_chats(limit: usize) -> Result<()> {
    chat::list_chats(limit).await
}

/// Read messages from a chat (native Teams API)
pub async fn read_messages(chat_id: &str, limit: usize) -> Result<()> {
    chat::read_messages(chat_id, limit).await
}

/// Send a message to a chat (native Teams API)
pub async fn send_message(to: &str, message: &str) -> Result<()> {
    chat::send_message(to, message).await
}

/// Get current presence status
pub async fn get_presence() -> Result<()> {
    presence::get_presence().await
}

/// Set presence status
pub async fn set_presence(status: &str) -> Result<()> {
    presence::set_presence(status).await
}

/// Show current user info
pub async fn whoami() -> Result<()> {
    me::whoami().await
}

/// List joined teams and their channels
pub async fn list_teams() -> Result<()> {
    teams::list_teams().await
}
