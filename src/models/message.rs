//! Message-related models

use serde::{Deserialize, Serialize};

/// Message body content type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentType {
    Text,
    Html,
}

/// Message body
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBody {
    pub content_type: ContentType,
    pub content: String,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub body: MessageBody,
    pub created_date_time: String,
    pub from: Option<MessageFrom>,
}

/// Message sender
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFrom {
    pub user: Option<UserIdentity>,
}

/// User identity in message context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserIdentity {
    pub id: String,
    pub display_name: Option<String>,
}
