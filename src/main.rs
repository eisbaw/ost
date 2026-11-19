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

    /// Channel moderation (Graph beta read, middle-tier write).
    /// Bare: show who can post/reply. With flags: set, then read back.
    /// --posts everyone|everyoneExceptGuests|moderators.
    /// --replies everyone|authorAndModerators.
    ChannelModeration {
        /// Team id (from `teams` output)
        #[arg(long)]
        team: String,

        /// Channel id (from `teams` output)
        #[arg(long)]
        channel: String,

        /// Who can start new posts
        #[arg(long)]
        posts: Option<String>,

        /// Who can reply to posts
        #[arg(long)]
        replies: Option<String>,
    },

    /// Team privacy switch (backing M365 group visibility).
    /// Bare: show Private/Public. --set private|public: flip, then read back.
    TeamVisibility {
        /// Team id (from `teams` output)
        #[arg(long)]
        team: String,

        /// Flip to `private` (invite-only) or `public`
        #[arg(long)]
        set: Option<String>,
    },
    /// Team lifecycle (team-admin): rename/re-describe, picture,
    /// archive/restore, delete. At least one action flag is required;
    /// --delete is irreversible (confirm first).
    Team {
        /// Team id (from `teams` output)
        #[arg(long)]
        team: String,

        /// Rename the team (PATCH /groups/{id} displayName)
        #[arg(long)]
        rename: Option<String>,

        /// Re-describe the team (PATCH /groups/{id} description)
        #[arg(long)]
        description: Option<String>,

        /// Set the team picture from an image file (PUT photo/$value)
        #[arg(long, value_name = "PATH")]
        photo: Option<String>,

        /// Archive the team (async server-side; re-list to confirm)
        #[arg(long)]
        archive: bool,

        /// Restore an archived team
        #[arg(long)]
        unarchive: bool,

        /// Delete the team (irreversible)
        #[arg(long)]
        delete: bool,
    },

    /// List shared files in a chat or channel
    Files {
        /// Chat or channel ID (from `chats` / `teams` output)
        chat_id: String,

        /// Maximum number of files to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Download a shared file by drive+item id
    FilesDownload {
        /// Drive ID (from `files` output)
        drive_id: String,

        /// DriveItem ID
        item_id: String,

        /// Destination file path
        out: String,
    },

    /// Upload a local file to a chat or channel (<4 MB)
    FilesUpload {
        /// Chat or channel ID (from `chats` / `teams` output)
        #[arg(short, long)]
        to: String,

        /// Local file path
        path: String,
    },

    /// Create a view-only sharing link for a shared file
    FilesLink {
        /// Drive ID (from `files` list output)
        drive_id: String,

        /// DriveItem ID
        item_id: String,

        /// Link scope: organization (org-only, default) or anonymous
        #[arg(long, default_value = "organization")]
        scope: String,
    },

    /// Search OneDrive files by name/content (om-jb-filesearch)
    FileSearch {
        /// Free-text query
        query: String,

        /// Maximum number of files to show
        #[arg(short, long, default_value = "25")]
        limit: usize,
    },

    /// Search the directory for people (om-jb-filesearch)
    PeopleSearch {
        /// Free-text query (matches display name)
        query: String,

        /// Maximum number of people to show
        #[arg(short, long, default_value = "25")]
        limit: usize,
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
        Commands::ChannelModeration {
            team,
            channel,
            posts,
            replies,
        } => {
            if posts.is_none() && replies.is_none() {
                api::show_moderation(&team, &channel).await?;
            } else {
                tracing::info!("Setting channel moderation...");
                api::set_moderation(&team, &channel, posts.as_deref(), replies.as_deref()).await?;
            }
        }
        Commands::TeamVisibility { team, set } => {
            match set.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
                None => api::show_visibility(&team).await?,
                Some(which) if which.is_empty() => api::show_visibility(&team).await?,
                Some(which) if which == "private" || which == "public" => {
                    tracing::info!("Setting team visibility...");
                    api::set_visibility(&team, which == "private").await?;
                }
                Some(other) => anyhow::bail!("--set must be 'private' or 'public' (got '{}')", other),
            }
        }
        Commands::Team {
            team,
            rename,
            description,
            photo,
            archive,
            unarchive,
            delete,
        } => {
            if archive && unarchive {
                anyhow::bail!("--archive and --unarchive are exclusive");
            }
            if delete && (rename.is_some() || description.is_some() || photo.is_some() || archive || unarchive) {
                anyhow::bail!("--delete is exclusive with all other team actions");
            }
            let mut acted = false;
            if rename.is_some() || description.is_some() {
                tracing::info!("Updating team...");
                api::update_team(&team, rename.as_deref(), description.as_deref()).await?;
                acted = true;
            }
            if let Some(path) = photo {
                tracing::info!("Setting team photo...");
                api::set_team_photo(&team, &path).await?;
                acted = true;
            }
            if archive {
                tracing::info!("Archiving team...");
                api::archive_team(&team).await?;
                acted = true;
            }
            if unarchive {
                tracing::info!("Unarchiving team...");
                api::unarchive_team(&team).await?;
                acted = true;
            }
            if delete {
                tracing::info!("Deleting team...");
                api::delete_team(&team).await?;
                acted = true;
            }
            if !acted {
                anyhow::bail!("team: pass one of --rename/--description/--photo/--archive/--unarchive/--delete");
            }
        }


        Commands::Files { chat_id, limit } => {
            tracing::info!("Fetching shared files...");
            api::list_files(&chat_id, limit).await?;
        }
        Commands::FilesDownload {
            drive_id,
            item_id,
            out,
        } => {
            api::download_file(&drive_id, &item_id, &out).await?;
        }
        Commands::FilesUpload { to, path } => {
            tracing::info!("Uploading file...");
            api::upload_file(&to, &path).await?;
        }
        Commands::FilesLink {
            drive_id,
            item_id,
            scope,
        } => {
            api::create_link(&drive_id, &item_id, &scope).await?;
        }
        Commands::FileSearch { query, limit } => {
            tracing::info!("Searching files...");
            api::search_files(&query, limit).await?;
        }
        Commands::PeopleSearch { query, limit } => {
            tracing::info!("Searching people...");
            api::search_people(&query, limit).await?;
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
