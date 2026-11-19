//! Teams CLI - Lightweight Microsoft Teams client
//!
//! A terminal-based Teams client for Linux.

mod api;
mod auth;
mod calling;
mod config;
mod models;
mod trouter;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "teams-cli")]
#[command(about = "Lightweight CLI client for Microsoft Teams", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Microsoft Teams
    Login {
        /// Force interactive login even if cached token exists
        #[arg(short, long)]
        force: bool,
    },

    /// Log out and clear cached credentials
    Logout,

    /// Show current authentication status
    Status,

    /// List recent chats
    Chats {
        /// Maximum number of chats to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Read messages from a chat
    Read {
        /// Chat thread ID (from `chats` output)
        chat_id: String,

        /// Maximum number of messages to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Send a message
    Send {
        /// Chat thread ID (from `chats` output)
        #[arg(short, long)]
        to: String,

        /// Message content
        message: String,
    },

    /// Edit one own message
    Edit {
        /// Chat thread ID (from `chats` output)
        #[arg(short, long)]
        to: String,

        /// Server message id (from `read` verbose logs)
        #[arg(long)]
        message_id: String,

        /// Replacement text
        message: String,
    },

    /// Delete one own message
    Delete {
        /// Chat thread ID (from `chats` output)
        #[arg(short, long)]
        to: String,

        /// Server message id (from `read` verbose logs)
        #[arg(long)]
        message_id: String,
    },

    /// Add an emoji reaction to a message (one of 👍 ❤️ 😂 😮 😢 😠)
    React {
        /// Chat thread ID (from `chats` output)
        #[arg(short, long)]
        to: String,

        /// Server message id (from `read` JSON via library)
        #[arg(long)]
        message_id: String,

        /// Picker emoji (e.g. 👍)
        emoji: String,

        /// Remove instead of add
        #[arg(long)]
        remove: bool,
    },

    /// List joined teams and their channels
    Teams,

    /// Create a team (private standard) via the Teams middle tier
    TeamsCreate {
        /// Team display name
        #[arg(long)]
        name: String,

        /// Team description (blank when omitted)
        #[arg(long)]
        description: Option<String>,
    },

    /// Team roster (Graph /teams/{id}/members).
    /// Bare: list members + owners. --owners: owners only.
    /// --add <user-id-or-upn> [--owner]: add. --remove <membership-id>: remove.
    Members {
        /// Team id (from `teams` output)
        #[arg(long)]
        team: String,

        /// Show owners only (list mode)
        #[arg(long)]
        owners: bool,

        /// Add this user id or UPN to the team
        #[arg(long)]
        add: Option<String>,

        /// Grant the owner role with --add
        #[arg(long)]
        owner: bool,

        /// Remove this membership id (from list output) from the team
        #[arg(long)]
        remove: Option<String>,
    },

    /// Show current user info (verify auth works)
    Whoami,

    /// Connect to Trouter WebSocket push service
    Trouter,

    /// Get/set presence status
    Presence {
        /// New status: available, busy, dnd, away, offline
        #[arg(short, long)]
        set: Option<String>,
    },

    /// List a meeting's breakout rooms (MiddleTier, read-only)
    BreakoutRooms {
        /// Meeting coords JSON (tenant_id, organizer_id, thread_id,
        /// message_id, additional_message_id?)
        coords: String,
    },

    /// Create a meeting's breakout rooms (MiddleTier)
    BreakoutCreate {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Room count (1..=50)
        #[arg(short, long, default_value = "4")]
        count: usize,

        /// Preconfiguration JSON array (participants + room keys)
        #[arg(long)]
        preconfig: Option<String>,

        /// Settings JSON (room_duration, allow_* flags, managers)
        #[arg(long)]
        settings: Option<String>,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Add one breakout room (MiddleTier)
    BreakoutAdd {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// New room name
        #[arg(short, long)]
        name: String,

        /// Participants JSON array
        #[arg(short, long, default_value = "[]")]
        participants: String,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Read one breakout room (MiddleTier, read-only)
    BreakoutGet {
        /// Room coords JSON (see breakout-rooms)
        coords: String,
    },

    /// Update one breakout room: rename + assign/move roster (MiddleTier)
    BreakoutUpdate {
        /// Room coords JSON (see breakout-rooms)
        coords: String,

        /// Room name
        #[arg(short, long)]
        name: String,

        /// Participants JSON array (the room's full new roster)
        #[arg(short, long, default_value = "[]")]
        participants: String,

        /// Restrict chat access in the room
        #[arg(long)]
        restrict_chat_access: bool,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Delete one breakout room (MiddleTier)
    BreakoutDelete {
        /// Room coords JSON (see breakout-rooms)
        coords: String,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Delete all breakout rooms (MiddleTier)
    BreakoutDeleteAll {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Rooms-references JSON body (omit for a bodyless delete)
        #[arg(long)]
        rooms_refs: Option<String>,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Bulk assign/move participants across rooms (MiddleTier)
    BreakoutAssign {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Rooms JSON array (coordinates, roomName, participants,
        /// restrictChatAccess each)
        #[arg(short, long)]
        rooms: String,

        /// Total participant count (switches to the v2 shape)
        #[arg(long)]
        total_participants: Option<u64>,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Open 1..n breakout rooms (MiddleTier)
    BreakoutOpen {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Room coordinates JSON array
        #[arg(short, long)]
        rooms: String,

        /// Use the new timer flow
        #[arg(long)]
        new_timer_flow: bool,

        /// Total participant count
        #[arg(long)]
        total_participants: Option<u64>,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Close 1..n breakout rooms (MiddleTier)
    BreakoutClose {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Room coordinates JSON array
        #[arg(short, long)]
        rooms: String,

        /// Total participant count
        #[arg(long)]
        total_participants: Option<u64>,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Read a meeting's breakout settings (MiddleTier, read-only)
    BreakoutSettings {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,
    },

    /// Write a meeting's breakout settings (MiddleTier)
    BreakoutSettingsSet {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Settings JSON (allow_users_to_select_breakout_room,
        /// room_duration, managers, allow_join_back_to_main_meeting,
        /// allow_move_participants_automatically)
        #[arg(short, long)]
        settings: String,

        /// MT requestVersion (capture pin; omit when unknown)
        #[arg(long)]
        version: Option<String>,
    },

    /// Read a meeting's assignable breakout users (MiddleTier, read-only)
    BreakoutAssignees {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Calendar event id filter
        #[arg(long)]
        calendar_event_id: Option<String>,

        /// iCal UID filter
        #[arg(long)]
        ical_uid: Option<String>,

        /// Event type filter
        #[arg(long)]
        event_type: Option<String>,

        /// Team id filter
        #[arg(long)]
        team_id: Option<String>,

        /// Max nest level
        #[arg(long)]
        max_nest_level: Option<String>,

        /// User threshold
        #[arg(long)]
        user_threshold: Option<String>,
    },

    /// Announce a message into breakout rooms (MiddleTier)
    BreakoutAnnounce {
        /// Meeting coords JSON (see breakout-rooms)
        coords: String,

        /// Room thread ids, comma-separated
        #[arg(short, long)]
        rooms: String,

        /// Message text
        #[arg(short, long)]
        message: String,
    },

    /// Place a test call to yourself (self-call)
    CallTest {
        /// Duration in seconds to keep the call active
        #[arg(short, long, default_value = "15")]
        duration: u64,

        /// Enable call recording via recorder bot injection
        #[arg(long)]
        record: bool,

        /// Call the Echo / Call Quality Tester bot instead of channel meeting
        #[arg(long)]
        echo: bool,

        /// 1:1 chat thread ID to call (e.g., 19:guid1_guid2@unq.gbl.spaces)
        #[arg(long)]
        thread: Option<String>,

        /// Enable camera capture (V4L2) for video send (requires video-capture feature)
        #[arg(long)]
        camera: bool,

        /// Enable video display window for received video (requires video-capture feature)
        #[arg(long)]
        display: bool,

        /// Use 1kHz test tone instead of real microphone (debug mode)
        #[arg(long)]
        tone: bool,
    },

    /// Test microphone capture: record 3 seconds then play back
    #[cfg(feature = "audio")]
    MicTest,

    /// Test camera capture: record 3 seconds then play back in SDL2 window
    #[cfg(feature = "video-capture")]
    CamTest,

    /// Launch the terminal user interface
    Tui,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging differently for TUI vs CLI mode.
    // TUI mode captures logs to a buffer (displayed in debug pane).
    // CLI mode logs to stderr as usual.
    let filter_str = if cli.verbose { "debug" } else { "info" };

    if matches!(cli.command, Commands::Tui) {
        // TUI mode: capture logs to a buffer for in-TUI display.
        let log_buffer = tui::LogBuffer::new();
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| filter_str.into()),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(log_buffer.clone()),
            )
            .init();

        return tui::run(log_buffer).await;
    }

    // CLI mode: log to stderr.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| filter_str.into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    match cli.command {
        Commands::Login { force } => {
            tracing::info!("Starting authentication flow...");
            auth::login(force).await?;
        }
        Commands::Logout => {
            tracing::info!("Logging out...");
            auth::logout().await?;
        }
        Commands::Status => {
            auth::status().await?;
        }
        Commands::Teams => {
            api::list_teams().await?;
        }
        Commands::TeamsCreate { name, description } => {
            tracing::info!("Creating team...");
            api::create_team(&name, description.as_deref()).await?;
        }
        Commands::Members {
            team,
            owners,
            add,
            owner,
            remove,
        } => {
            if let Some(user) = add {
                if remove.is_some() {
                    anyhow::bail!("--add and --remove are exclusive");
                }
                tracing::info!("Adding team member...");
                api::add_team_member(&team, &user, owner).await?;
            } else if let Some(member) = remove {
                tracing::info!("Removing team member...");
                api::remove_team_member(&team, &member).await?;
            } else {
                api::list_team_members(&team, owners).await?;
            }
        }
        Commands::Whoami => {
            api::whoami().await?;
        }
        Commands::Chats { limit } => {
            tracing::info!("Fetching chats...");
            api::list_chats(limit).await?;
        }
        Commands::Read { chat_id, limit } => {
            api::read_messages(&chat_id, limit).await?;
        }
        Commands::Send { to, message } => {
            tracing::info!("Sending message...");
            api::send_message(&to, &message).await?;
        }
        Commands::Edit {
            to,
            message_id,
            message,
        } => {
            tracing::info!("Editing message...");
            api::edit_message(&to, &message_id, &message).await?;
        }
        Commands::Delete { to, message_id } => {
            tracing::info!("Deleting message...");
            api::delete_message(&to, &message_id).await?;
        }
        Commands::React {
            to,
            message_id,
            emoji,
            remove,
        } => {
            api::react(&to, &message_id, &emoji, remove).await?;
        }
        Commands::Trouter => {
            trouter::connect_and_run().await?;
        }
        Commands::BreakoutRooms { coords } => {
            api::show_breakout_rooms(&coords).await?;
        }
        Commands::BreakoutCreate { coords, count, preconfig, settings, version } => {
            api::show_breakout_create(&coords, count, preconfig.as_deref(), settings.as_deref(), version.as_deref())
                .await?;
        }
        Commands::BreakoutAdd { coords, name, participants, version } => {
            api::show_breakout_add(&coords, &name, &participants, version.as_deref()).await?;
        }
        Commands::BreakoutGet { coords } => {
            api::show_breakout_get(&coords).await?;
        }
        Commands::BreakoutUpdate { coords, name, participants, restrict_chat_access, version } => {
            api::show_breakout_update(&coords, &name, &participants, restrict_chat_access, version.as_deref()).await?;
        }
        Commands::BreakoutDelete { coords, version } => {
            api::show_breakout_delete(&coords, version.as_deref()).await?;
        }
        Commands::BreakoutDeleteAll { coords, rooms_refs, version } => {
            api::show_breakout_delete_all(&coords, rooms_refs.as_deref(), version.as_deref()).await?;
        }
        Commands::BreakoutAssign { coords, rooms, total_participants, version } => {
            api::show_breakout_assign(&coords, &rooms, total_participants, version.as_deref()).await?;
        }
        Commands::BreakoutOpen { coords, rooms, new_timer_flow, total_participants, version } => {
            api::show_breakout_open(&coords, &rooms, new_timer_flow, total_participants, version.as_deref()).await?;
        }
        Commands::BreakoutClose { coords, rooms, total_participants, version } => {
            api::show_breakout_close(&coords, &rooms, total_participants, version.as_deref()).await?;
        }
        Commands::BreakoutSettings { coords } => {
            api::show_breakout_settings(&coords).await?;
        }
        Commands::BreakoutSettingsSet { coords, settings, version } => {
            api::show_breakout_settings_set(&coords, &settings, version.as_deref()).await?;
        }
        Commands::BreakoutAssignees {
            coords,
            calendar_event_id,
            ical_uid,
            event_type,
            team_id,
            max_nest_level,
            user_threshold,
        } => {
            api::show_breakout_assignees(
                &coords,
                calendar_event_id.as_deref(),
                ical_uid.as_deref(),
                event_type.as_deref(),
                team_id.as_deref(),
                max_nest_level.as_deref(),
                user_threshold.as_deref(),
            )
            .await?;
        }
        Commands::BreakoutAnnounce { coords, rooms, message } => {
            api::show_breakout_announce(&coords, &rooms, &message).await?;
        }
        Commands::CallTest {
            duration,
            record,
            echo,
            thread,
            camera,
            display,
            tone,
        } => {
            calling::call_test::run_call_test(
                duration, record, echo, thread, camera, display, tone,
            )
            .await?;
        }
        #[cfg(feature = "audio")]
        Commands::MicTest => {
            calling::audio::mic_test()?;
        }
        #[cfg(feature = "video-capture")]
        Commands::CamTest => {
            calling::camera::cam_test()?;
        }
        Commands::Presence { set } => match set {
            Some(status) => {
                tracing::info!("Setting presence to {}...", status);
                api::set_presence(&status).await?;
            }
            None => {
                api::get_presence().await?;
            }
        },
        // TUI is handled above with early return.
        Commands::Tui => unreachable!(),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakout_verbs_parse() {
        let coords = r#"{"tenant_id":"t","organizer_id":"o","thread_id":"19:x","message_id":"1"}"#;
        match Cli::try_parse_from(["teams-cli", "breakout-rooms", coords]).expect("rooms parses").command {
            Commands::BreakoutRooms { coords: c } => assert!(c.contains("tenant_id")),
            _ => panic!("wrong command parsed"),
        }
        match Cli::try_parse_from([
            "teams-cli", "breakout-create", coords, "--count", "6", "--version", "3",
        ])
        .expect("create parses")
        .command
        {
            Commands::BreakoutCreate { count, version, preconfig, .. } => {
                assert_eq!(count, 6);
                assert_eq!(version.as_deref(), Some("3"));
                assert_eq!(preconfig, None);
            }
            _ => panic!("wrong command parsed"),
        }
        match Cli::try_parse_from([
            "teams-cli", "breakout-update", coords, "--name", "R", "--restrict-chat-access",
        ])
        .expect("update parses")
        .command
        {
            Commands::BreakoutUpdate { name, restrict_chat_access, .. } => {
                assert_eq!(name, "R");
                assert!(restrict_chat_access);
            }
            _ => panic!("wrong command parsed"),
        }
        match Cli::try_parse_from(["teams-cli", "breakout-announce", coords, "--rooms", "a,b", "--message", "hi"])
            .expect("announce parses")
            .command
        {
            Commands::BreakoutAnnounce { rooms, message, .. } => {
                assert_eq!(rooms, "a,b");
                assert_eq!(message, "hi");
            }
            _ => panic!("wrong command parsed"),
        }
    }

    #[test]
    fn breakout_verbs_require_their_args() {
        assert!(Cli::try_parse_from(["teams-cli", "breakout-rooms"]).is_err());
        assert!(Cli::try_parse_from(["teams-cli", "breakout-add", "{}"]).is_err());
        assert!(Cli::try_parse_from(["teams-cli", "breakout-assign", "{}", "--rooms", "[]"]).is_ok());
    }
}
