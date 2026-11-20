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

    /// List joined teams and their channels
    Teams,

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

    /// List my whiteboards (legacy Azure boards)
    WbList,

    /// Create one whiteboard (prints the created board)
    WbCreate {
        /// Board title (blank when omitted)
        #[arg(long)]
        title: Option<String>,
    },

    /// Open one whiteboard (prints the board)
    WbShow {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// Rename one whiteboard (prints the updated board)
    WbRename {
        /// Board id (from `wb-list` output)
        board_id: String,

        /// New board title
        #[arg(long)]
        title: String,
    },

    /// Delete one whiteboard
    WbDelete {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// Mint a share link on one whiteboard (prints the link)
    WbShare {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// Fetch one whiteboard's share link (prints the link)
    WbShareShow {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// Revoke one whiteboard's share link
    WbUnshare {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// List one whiteboard's collaborators
    WbCollaborators {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// Read my permissions on one whiteboard
    WbMyPerms {
        /// Board id (from `wb-list` output)
        board_id: String,
    },

    /// Redeem a legacy whiteboard share token
    WbRedeem {
        /// Share token (from `wb-share` output)
        token: String,
    },

    /// Read one whiteboard content page (ink-sync read path)
    WbContent {
        /// Board id (from `wb-list` output)
        board_id: String,

        /// Next-page cursor (from a previous `wb-content` run)
        #[arg(long)]
        cursor: Option<String>,
    },

    /// Join one board file's delta session (ink-sync channel open)
    WbJoin {
        /// SharePoint site URL (from the drive locator)
        #[arg(long)]
        site: String,

        /// Drive id
        #[arg(long)]
        drive: String,

        /// Board file item id
        #[arg(long)]
        item: String,

        /// Full Authorization header value for the join body
        #[arg(long)]
        auth: String,
    },

    /// Print a board's Teams tab-config URLs (no network)
    WbTabUrl {
        /// Azure board id (with --token)
        #[arg(long)]
        board: Option<String>,

        /// Azure share token (with --board)
        #[arg(long)]
        token: Option<String>,

        /// Fluid board file web URL
        #[arg(long)]
        web_url: Option<String>,
    },

    /// Print a board's meeting participant URL (no network)
    WbMeetingUrl {
        /// Fluid board file web URL
        #[arg(long)]
        web_url: String,

        /// Participant may edit (else view-only)
        #[arg(long)]
        can_edit: bool,

        /// Proof token (ad-hoc access)
        #[arg(long)]
        proof_token: Option<String>,

        /// Fluid file id
        #[arg(long)]
        fluid_file_id: Option<String>,

        /// Annotation board
        #[arg(long)]
        annotation: bool,
    },
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
        Commands::WbList => {
            tracing::info!("Fetching whiteboards...");
            api::list_whiteboards().await?;
        }
        Commands::WbCreate { title } => {
            tracing::info!("Creating whiteboard...");
            api::create_whiteboard(title.as_deref()).await?;
        }
        Commands::WbShow { board_id } => {
            tracing::info!("Fetching whiteboard...");
            api::show_whiteboard(&board_id).await?;
        }
        Commands::WbRename { board_id, title } => {
            tracing::info!("Renaming whiteboard...");
            api::rename_whiteboard(&board_id, &title).await?;
        }
        Commands::WbDelete { board_id } => {
            tracing::info!("Deleting whiteboard...");
            api::delete_whiteboard(&board_id).await?;
        }
        Commands::WbShare { board_id } => {
            tracing::info!("Sharing whiteboard...");
            api::share_whiteboard(&board_id).await?;
        }
        Commands::WbShareShow { board_id } => {
            tracing::info!("Fetching whiteboard share link...");
            api::show_share_link(&board_id).await?;
        }
        Commands::WbUnshare { board_id } => {
            tracing::info!("Revoking whiteboard share link...");
            api::revoke_share_link(&board_id).await?;
        }
        Commands::WbCollaborators { board_id } => {
            tracing::info!("Fetching whiteboard collaborators...");
            api::show_collaborators(&board_id).await?;
        }
        Commands::WbMyPerms { board_id } => {
            tracing::info!("Fetching whiteboard permissions...");
            api::show_my_permissions(&board_id).await?;
        }
        Commands::WbRedeem { token } => {
            tracing::info!("Redeeming whiteboard share token...");
            api::redeem_whiteboard(&token).await?;
        }
        Commands::WbContent { board_id, cursor } => {
            tracing::info!("Reading whiteboard content...");
            api::show_content(&board_id, cursor.as_deref()).await?;
        }
        Commands::WbJoin { site, drive, item, auth } => {
            tracing::info!("Joining whiteboard session...");
            api::join_whiteboard(&site, &drive, &item, &auth).await?;
        }
        Commands::WbTabUrl { board, token, web_url } => {
            api::show_tab_urls(board.as_deref(), token.as_deref(), web_url.as_deref()).await?;
        }
        Commands::WbMeetingUrl { web_url, can_edit, proof_token, fluid_file_id, annotation } => {
            api::show_meeting_url(
                &web_url,
                can_edit,
                proof_token.as_deref(),
                fluid_file_id.as_deref(),
                annotation,
            )
            .await?;
        }
        // TUI is handled above with early return.
        Commands::Tui => unreachable!(),
    }

    Ok(())
}
