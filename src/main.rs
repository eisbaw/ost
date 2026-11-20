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

    /// Copilot chat probe (ChatHub transport, free tier first).
    /// `--print-frames`/`--replay` are offline; `--chat`/`--license-status`
    /// need tenant recovery (phase-2 P0/P4) and are never run blind.
    Copilot {
        /// Print the built handshake/ping/invocation frames for PROMPT (offline)
        #[arg(long)]
        print_frames: Option<String>,

        /// Parse one recorded server record and print the event (offline)
        #[arg(long)]
        replay: Option<String>,

        /// Run one live chat turn (phase 2 only)
        #[arg(long)]
        chat: Option<String>,

        /// Fetch the Taiger licence status (phase 2 only)
        #[arg(long)]
        license_status: bool,

        /// Token family: sydney (mapped) or substrate (P4 probe)
        #[arg(long, default_value = "sydney")]
        token_family: String,

        /// Model selector tone (capture-pin; default Magic = Auto)
        #[arg(long, default_value = "Magic")]
        tone: String,

        /// Extra optionsSets flags, repeatable (default: none = free tier)
        #[arg(long)]
        option_set: Vec<String>,

        /// Hub env: default, prod, sdf, msit, gcch, dod, teams2024
        #[arg(long, default_value = "default")]
        hub: String,
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
        Commands::Copilot {
            print_frames,
            replay,
            chat,
            license_status,
            token_family,
            tone,
            option_set,
            hub,
        } => {
            copilot_probe(print_frames, replay, chat, license_status, &token_family, &tone, option_set, &hub)
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

/// Copilot chat probe. Offline modes (`--print-frames`, `--replay`)
/// never touch the network; live modes (`--chat`, `--license-status`)
/// are phase-2 only (tenant recovery + owner brief). Tokens come from
/// the stacked token broker (#75); upstream is single-account, so no
/// profile lookup (vendored profiles ride #50, still open).
#[allow(clippy::too_many_arguments)]
async fn copilot_probe(
    print_frames: Option<String>,
    replay: Option<String>,
    chat: Option<String>,
    license_status: bool,
    token_family: &str,
    tone: &str,
    option_set: Vec<String>,
    hub: &str,
) -> Result<()> {
    let mut turn = api::ChatTurn::free_tier("");
    turn.tone = tone.to_string();
    turn.options_sets = option_set;
    if let Some(prompt) = print_frames {
        turn.text = prompt;
        let inv = api::chat_invocation(&turn, "0");
        println!("f0 handshake: {}", api::handshake_request().replace('\u{1e}', "␞"));
        println!("f2 ping: {}", api::ping_frame().replace('\u{1e}', "␞"));
        println!("f3 invocation: {}", api::frame(&inv).replace('\u{1e}', "␞"));
        let q = api::HubQuery::default();
        let url = api::hub_url_2026(api::HUB_DEFAULT, &turn.conversation_id, "REDACTED", "teamshub", "teamshub", &q);
        println!("url: {}", url);
        return Ok(());
    }
    if let Some(record) = replay {
        for r in api::split_frames(&record) {
            println!("{:?}", api::parse_server_record(r));
        }
        return Ok(());
    }
    if license_status {
        let scope = format!("{}/.default", api::TAIGER_BASE);
        let grant = auth::oauth::token_for_scope(&scope).await?;
        let (oid, tid) = api::jwt_oid_tid(&grant.access_token)
            .ok_or_else(|| anyhow::anyhow!("token has no oid/tid"))?;
        let s = api::license_status(&grant.access_token, &oid, &tid).await;
        println!("status: {}", s.status);
        println!("licensed: {}", api::licensed(&s.status));
        if let Some(r) = api::license_reason(&s.status) {
            println!("reason: {}", r);
        }
        if let Some(e) = s.error_text() {
            println!("error: {}", e);
        }
        return Ok(());
    }
    if let Some(prompt) = chat {
        turn.text = prompt;
        let token = match token_family {
            "substrate" => api::substrate_token_for().await?,
            _ => api::sydney_token_for().await?,
        };
        let url = if hub == "teams2024" {
            let (oid, tid) =
                api::jwt_oid_tid(&token).ok_or_else(|| anyhow::anyhow!("token has no oid/tid"))?;
            api::hub_url_teams(&oid, &tid, &token)
        } else {
            let base = match hub {
                "prod" => api::HUB_PROD,
                "sdf" => api::HUB_SDF,
                "msit" => api::HUB_MSIT,
                "gcch" => api::HUB_GCCH,
                "dod" => api::HUB_DOD,
                _ => api::HUB_DEFAULT,
            };
            api::hub_url_2026(base, &turn.conversation_id, &token, "teamshub", "teamshub", &api::HubQuery::default())
        };
        let answer = api::run_turn(&url, &turn, |p| match p {
            api::TurnProgress::Text(t) => {
                print!("\r\x1b[2K{}", t.lines().last().unwrap_or(""));
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            api::TurnProgress::Progress(p) => tracing::info!("progress: {}", p),
            api::TurnProgress::Done(_) => {}
        })
        .await?;
        println!("\r\x1b[2K{}", answer.text);
        for s in &answer.suggestions {
            println!("suggestion: {}", s);
        }
        return Ok(());
    }
    anyhow::bail!("pick one: --print-frames | --replay | --chat | --license-status");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_probe_parses_offline_and_live_modes() {
        let cli = Cli::try_parse_from(["teams-cli", "copilot", "--print-frames", "hi"]).unwrap();
        match cli.command {
            Commands::Copilot { print_frames, tone, token_family, hub, .. } => {
                assert_eq!(print_frames.as_deref(), Some("hi"));
                assert_eq!(tone, "Magic");
                assert_eq!(token_family, "sydney");
                assert_eq!(hub, "default");
            }
            _ => panic!("wrong command parsed"),
        }
        let cli = Cli::try_parse_from([
            "teams-cli", "copilot", "--chat", "hi", "--token-family", "substrate",
            "--tone", "Chat", "--option-set", "a", "--option-set", "b", "--hub", "teams2024",
        ])
        .unwrap();
        match cli.command {
            Commands::Copilot { chat, token_family, tone, option_set, hub, .. } => {
                assert_eq!(chat.as_deref(), Some("hi"));
                assert_eq!(token_family, "substrate");
                assert_eq!(tone, "Chat");
                assert_eq!(option_set, vec!["a".to_string(), "b".to_string()]);
                assert_eq!(hub, "teams2024");
            }
            _ => panic!("wrong command parsed"),
        }
    }
}
