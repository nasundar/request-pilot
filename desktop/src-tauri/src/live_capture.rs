use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::Emitter;
use tokio::sync::{broadcast, Mutex, Notify};
use tokio_tungstenite::tungstenite::Message;

const LIVE_CAPTURE_PORT: u16 = 9718;

/// A captured HTTP request forwarded from the browser extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedRequest {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub request_headers: Vec<HeaderPair>,
    #[serde(default)]
    pub request_body: Option<String>,
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub response_headers: Vec<HeaderPair>,
    #[serde(default)]
    pub response_body: Option<String>,
    #[serde(default)]
    pub duration: Option<u64>,
    #[serde(default)]
    pub timestamp: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

/// Incoming message from the extension.
#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
#[serde(rename_all = "snake_case")]
enum IncomingMessage {
    Request { data: CapturedRequest },
    Pong,
    Connected,
}

/// Outgoing message to the extension.
#[derive(Debug, Serialize)]
#[serde(tag = "action")]
#[serde(rename_all = "snake_case")]
pub enum OutgoingMessage {
    SetMode { mode: String },
    Ping,
}

/// Shared state for the live capture server.
pub struct LiveCaptureState {
    /// Current capture mode: "off", "all", "filtered"
    pub mode: Mutex<String>,
    /// Channel to send commands to connected extension clients
    pub command_tx: broadcast::Sender<String>,
    /// Whether the server is running
    pub running: Mutex<bool>,
    /// Connection status
    pub connected: Mutex<bool>,
    /// Shutdown signal to wake the accept loop
    pub shutdown: Notify,
}

impl LiveCaptureState {
    pub fn new() -> Self {
        let (command_tx, _) = broadcast::channel(64);
        Self {
            mode: Mutex::new("off".to_string()),
            command_tx,
            running: Mutex::new(false),
            connected: Mutex::new(false),
            shutdown: Notify::new(),
        }
    }
}

/// Start the WebSocket server. Returns when the server is shut down.
pub async fn start_server(
    state: Arc<LiveCaptureState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // Check if already running
    {
        let mut running = state.running.lock().await;
        if *running {
            return Ok(());
        }
        *running = true;
    }

    let addr = format!("127.0.0.1:{}", LIVE_CAPTURE_PORT);
    let socket = tokio::net::TcpSocket::new_v4()
        .map_err(|e| format!("Failed to create socket: {}", e))?;
    socket
        .set_reuseaddr(true)
        .map_err(|e| format!("Failed to set SO_REUSEADDR: {}", e))?;
    socket
        .bind(addr.parse().unwrap())
        .map_err(|e| format!("Failed to bind WS server on {}: {}", addr, e))?;
    let listener = socket
        .listen(8)
        .map_err(|e| format!("Failed to listen on {}: {}", addr, e))?;

    let _ = app_handle.emit(
        "live-connection-status",
        serde_json::json!({ "status": "listening" }),
    );

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _addr)) => {
                        let state_clone = state.clone();
                        let app_clone = app_handle.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, state_clone, app_clone).await {
                                eprintln!("WS connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("WS accept error: {}", e);
                    }
                }
            }
            _ = state.shutdown.notified() => {
                break;
            }
        }
    }

    // Reset running flag so the server can be restarted
    *state.running.lock().await = false;
    *state.connected.lock().await = false;

    let _ = app_handle.emit(
        "live-connection-status",
        serde_json::json!({ "status": "stopped" }),
    );
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    state: Arc<LiveCaptureState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| format!("WS handshake failed: {}", e))?;

    let (mut write, mut read) = ws_stream.split();

    // Mark as connected
    *state.connected.lock().await = true;
    let _ = app_handle.emit(
        "live-connection-status",
        serde_json::json!({ "status": "connected" }),
    );

    // Send current mode to newly connected client
    let current_mode = state.mode.lock().await.clone();
    let mode_msg = serde_json::to_string(&OutgoingMessage::SetMode {
        mode: current_mode,
    })
    .unwrap_or_default();
    let _ = write.send(Message::Text(mode_msg.into())).await;

    // Subscribe to commands
    let mut command_rx = state.command_tx.subscribe();

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<IncomingMessage>(&text) {
                            Ok(IncomingMessage::Request { data }) => {
                                let _ = app_handle.emit("live-request", &data);
                            }
                            Ok(IncomingMessage::Pong) => {}
                            Ok(IncomingMessage::Connected) => {}
                            Err(e) => {
                                eprintln!("WS parse error: {} — raw: {}", e, &text[..text.len().min(200)]);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        eprintln!("WS read error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
            cmd = command_rx.recv() => {
                if let Ok(cmd_text) = cmd {
                    if write.send(Message::Text(cmd_text.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    // Mark as disconnected
    *state.connected.lock().await = false;
    let _ = app_handle.emit(
        "live-connection-status",
        serde_json::json!({ "status": "disconnected" }),
    );

    Ok(())
}

/// Stop the server by notifying the accept loop to exit.
pub async fn stop_server(state: &LiveCaptureState) {
    let was_running = *state.running.lock().await;
    *state.running.lock().await = false;
    *state.connected.lock().await = false;
    if was_running {
        state.shutdown.notify_one();
    }
}
