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

    /// Microsoft To Do lists and tasks (Graph /me/todo).
    /// Bare: show lists. --list <id>: tasks in that list.
    /// --add <title> --to <id>: create. --done <task> --to <id>: complete.
    Todo {
        /// Show tasks for this list id (default: show lists)
        #[arg(long)]
        list: Option<String>,

        /// Maximum tasks to show
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Create a task with this title (requires --to)
        #[arg(long)]
        add: Option<String>,

        /// Target list id for --add / --done
        #[arg(long)]
        to: Option<String>,

        /// Complete this task id (requires --to)
        #[arg(long)]
        done: Option<String>,
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
        Commands::Todo {
            list,
            limit,
            add,
            to,
            done,
        } => {
            if let Some(title) = add {
                let Some(list_id) = to else {
                    anyhow::bail!("--add requires --to <list-id>");
                };
                tracing::info!("Creating To Do task...");
                api::create_todo_task(&list_id, &title).await?;
            } else if let Some(task_id) = done {
                let Some(list_id) = to else {
                    anyhow::bail!("--done requires --to <list-id>");
                };
                tracing::info!("Completing To Do task...");
                api::complete_todo_task(&list_id, &task_id).await?;
            } else if let Some(list_id) = list {
                api::list_todo_tasks(&list_id, limit).await?;
            } else {
                api::list_todo_lists().await?;
            }
        }
        // TUI is handled above with early return.
        Commands::Tui => unreachable!(),
    }

    Ok(())
}
