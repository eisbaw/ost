//! Async backend: bridges the sync TUI event loop with async API calls.
//!
//! Uses an mpsc channel pair. The TUI sends `BackendCommand` values, and a
//! background tokio task executes them and sends `BackendResponse` values back.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;

use crate::api;
use crate::api::client::TeamsClient;

/// Commands sent from the TUI event loop to the async backend.
pub enum BackendCommand {
    LoadTeams,
    LoadChats { limit: usize },
    LoadMessages { chat_id: String, limit: usize },
    SendMessage { chat_id: String, message: String },
    LoadUserInfo,
    LoadPresence,
}

/// Responses from the async backend to the TUI.
pub enum BackendResponse {
    Teams(Result<Vec<api::TeamInfo>>),
    Chats(Result<Vec<api::ChatInfo>>),
    Messages {
        chat_id: String,
        result: Result<Vec<api::MessageInfo>>,
    },
    MessageSent(Result<()>),
    UserInfo(Result<api::UserInfo>),
    Presence(Result<api::PresenceInfo>),
    /// Initial client creation failed (auth issue).
    ClientError(String),
}

/// Handle for interacting with the backend from the TUI side.
pub struct Backend {
    cmd_tx: mpsc::UnboundedSender<BackendCommand>,
    resp_rx: mpsc::UnboundedReceiver<BackendResponse>,
}

impl Backend {
    /// Start the backend. Spawns a tokio task that processes commands.
    ///
    /// Returns the Backend handle for sending commands and receiving responses.
    pub fn start() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (resp_tx, resp_rx) = mpsc::unbounded_channel();

        tokio::spawn(backend_loop(cmd_rx, resp_tx));

        Self { cmd_tx, resp_rx }
    }

    /// Send a command to the backend (non-blocking).
    pub fn send(&self, cmd: BackendCommand) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::error!("Backend channel closed -- command dropped");
        }
    }

    /// Receive a response from the backend.
    ///
    /// Suspends until a response is available. Returns `None` only when the
    /// backend channel is permanently closed (all senders dropped).
    /// Designed to be used inside `tokio::select!`.
    pub async fn recv(&mut self) -> Option<BackendResponse> {
        self.resp_rx.recv().await
    }
}

/// Background loop that processes commands.
///
/// Creates a TeamsClient once and reuses it across all API calls.
/// If client creation fails, sends a ClientError response and exits.
async fn backend_loop(
    mut cmd_rx: mpsc::UnboundedReceiver<BackendCommand>,
    resp_tx: mpsc::UnboundedSender<BackendResponse>,
) {
    // Try to create the client. If this fails, the user needs to login first.
    let client = match TeamsClient::new().await {
        Ok(c) => Arc::new(c),
        Err(e) => {
            let _ = resp_tx.send(BackendResponse::ClientError(format!("{:#}", e)));
            return;
        }
    };

    while let Some(cmd) = cmd_rx.recv().await {
        let client = Arc::clone(&client);
        let resp_tx = resp_tx.clone();

        // Spawn each command as a separate task so we don't block the loop.
        tokio::spawn(async move {
            match cmd {
                BackendCommand::LoadTeams => {
                    let result = api::list_teams_data(&client).await;
                    let _ = resp_tx.send(BackendResponse::Teams(result));
                }
                BackendCommand::LoadChats { limit } => {
                    let result = api::list_chats_data(&client, limit).await;
                    let _ = resp_tx.send(BackendResponse::Chats(result));
                }
                BackendCommand::LoadMessages { chat_id, limit } => {
                    let result = api::read_messages_data(&client, &chat_id, limit).await;
                    let _ = resp_tx.send(BackendResponse::Messages { chat_id, result });
                }
                BackendCommand::SendMessage { chat_id, message } => {
                    let result = api::send_message_with_client(&client, &chat_id, &message).await;
                    let _ = resp_tx.send(BackendResponse::MessageSent(result));
                }
                BackendCommand::LoadUserInfo => {
                    let result = api::whoami_data(&client).await;
                    let _ = resp_tx.send(BackendResponse::UserInfo(result));
                }
                BackendCommand::LoadPresence => {
                    let result = api::get_presence_data(&client).await;
                    let _ = resp_tx.send(BackendResponse::Presence(result));
                }
            }
        });
    }
}
