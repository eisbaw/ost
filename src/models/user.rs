//! User-related models

use serde::{Deserialize, Serialize};

/// User presence availability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Availability {
    Available,
    AvailableIdle,
    Away,
    BeRightBack,
    Busy,
    BusyIdle,
    DoNotDisturb,
    Offline,
    PresenceUnknown,
}

/// User presence activity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Activity {
    Available,
    Away,
    BeRightBack,
    Busy,
    DoNotDisturb,
    InACall,
    InAConferenceCall,
    Inactive,
    InAMeeting,
    Offline,
    OffWork,
    OutOfOffice,
    PresenceUnknown,
    Presenting,
    UrgentInterruptionsOnly,
}

/// User presence
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub availability: Availability,
    pub activity: Activity,
}

/// User profile
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub display_name: Option<String>,
    pub user_principal_name: Option<String>,
    pub mail: Option<String>,
}
