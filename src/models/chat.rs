//! Chat-related models

use serde::{Deserialize, Serialize};

/// Chat type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatType {
    OneOnOne,
    Group,
    Meeting,
}

/// Chat entity
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub topic: Option<String>,
    pub chat_type: ChatType,
    pub created_date_time: Option<String>,
    pub last_updated_date_time: Option<String>,
}
