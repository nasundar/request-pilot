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
    /// Value defaults to empty string if missing (e.g. binary-only headers).
    #[serde(default)]
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
    #[allow(dead_code)]
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
        .bind(addr.parse::<std::net::SocketAddr>().map_err(|e| format!("Invalid address {}: {}", addr, e))?)
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
    use tokio_tungstenite::tungstenite::handshake::server::{
        Request as WsRequest, Response as WsResponse, ErrorResponse,
    };
    use tokio_tungstenite::tungstenite::http;

    // Validate Origin header — only accept browser extension origins
    let ws_stream = tokio_tungstenite::accept_hdr_async(
        stream,
        |req: &WsRequest, resp: WsResponse| -> Result<WsResponse, ErrorResponse> {
            if let Some(origin) = req.headers().get("origin") {
                let origin_str = origin.to_str().unwrap_or("");
                if origin_str.starts_with("chrome-extension://")
                    || origin_str.starts_with("moz-extension://")
                {
                    return Ok(resp);
                }
            }
            let mut err = http::Response::new(Some("Forbidden: invalid origin".to_string()));
            *err.status_mut() = http::StatusCode::FORBIDDEN;
            Err(err)
        },
    )
    .await
    .map_err(|e| format!("WS handshake rejected: {}", e))?;

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

    let mut msg_count: u64 = 0;

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        msg_count += 1;
                        match serde_json::from_str::<IncomingMessage>(&text) {
                            Ok(IncomingMessage::Request { data }) => {
                                let _ = app_handle.emit("live-request", &data);
                            }
                            Ok(IncomingMessage::Pong) => {}
                            Ok(IncomingMessage::Connected) => {}
                            Err(e) => {
                                let preview = &text[..text.len().min(300)];
                                eprintln!("WS parse error (msg #{}): {} — raw: {}", msg_count, e, preview);
                                let _ = app_handle.emit(
                                    "live-capture-error",
                                    serde_json::json!({
                                        "error": format!("{}", e),
                                        "preview": preview,
                                    }),
                                );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_request_message_full() {
        let json = r#"{"action":"request","data":{"url":"https://example.com/api","method":"GET","request_headers":[{"name":"Host","value":"example.com"},{"name":"Accept","value":"application/json"}],"request_body":null,"status_code":200,"response_headers":[{"name":"content-type","value":"text/html"}],"response_body":null,"duration":150,"timestamp":1234567890.123}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert_eq!(data.url, "https://example.com/api");
                assert_eq!(data.method, "GET");
                assert_eq!(data.request_headers.len(), 2);
                assert_eq!(data.request_headers[0].name, "Host");
                assert_eq!(data.status_code, Some(200));
                assert_eq!(data.duration, Some(150));
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn parse_request_message_minimal() {
        // Extension may send minimal data for failed requests
        let json = r#"{"action":"request","data":{"url":"https://api.test/health","method":"POST"}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert_eq!(data.url, "https://api.test/health");
                assert_eq!(data.method, "POST");
                assert!(data.request_headers.is_empty());
                assert!(data.request_body.is_none());
                assert!(data.status_code.is_none());
                assert!(data.duration.is_none());
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn parse_request_with_nulls() {
        let json = r#"{"action":"request","data":{"url":"https://x.com","method":"DELETE","request_headers":[],"request_body":null,"status_code":null,"response_headers":[],"response_body":null,"duration":null,"timestamp":null}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert_eq!(data.method, "DELETE");
                assert!(data.status_code.is_none());
                assert!(data.duration.is_none());
                assert!(data.timestamp.is_none());
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn parse_header_missing_value() {
        // Headers with binaryValue (no string value) should default to empty
        let json = r#"{"name":"x-binary-header"}"#;
        let hp: HeaderPair = serde_json::from_str(json).unwrap();
        assert_eq!(hp.name, "x-binary-header");
        assert_eq!(hp.value, "");
    }

    #[test]
    fn parse_connected_message() {
        let json = r#"{"action":"connected"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, IncomingMessage::Connected));
    }

    #[test]
    fn parse_pong_message() {
        let json = r#"{"action":"pong"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, IncomingMessage::Pong));
    }

    #[test]
    fn serialize_set_mode() {
        let msg = OutgoingMessage::SetMode {
            mode: "all".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""action":"set_mode""#));
        assert!(json.contains(r#""mode":"all""#));
    }

    #[test]
    fn parse_request_with_body() {
        let json = r#"{"action":"request","data":{"url":"https://api.test/users","method":"POST","request_headers":[{"name":"Content-Type","value":"application/json"}],"request_body":"{\"name\":\"test\"}","status_code":201,"response_headers":[],"response_body":"{\"id\":1}","duration":42,"timestamp":1700000000.0}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert_eq!(data.request_body, Some(r#"{"name":"test"}"#.to_string()));
                assert_eq!(data.response_body, Some(r#"{"id":1}"#.to_string()));
                assert_eq!(data.status_code, Some(201));
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn parse_request_status_code_zero() {
        // statusCode 0 means error/no response
        let json = r#"{"action":"request","data":{"url":"https://dead.host","method":"GET","status_code":0}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert_eq!(data.status_code, Some(0));
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn reject_unknown_action() {
        let json = r#"{"action":"unknown_action","data":{}}"#;
        assert!(serde_json::from_str::<IncomingMessage>(json).is_err());
    }

    #[test]
    fn parse_request_no_response_body_field() {
        // Live capture sends requests without response_body
        let json = r#"{"action":"request","data":{"url":"https://prom.azure.com/api/v1/query_range","method":"POST","request_headers":[{"name":"Authorization","value":"Bearer token123"}],"request_body":"query=up&start=1700000000&end=1700003600&step=60","status_code":200,"response_headers":[{"name":"content-type","value":"application/json"}],"duration":320,"timestamp":1700000000.5}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert_eq!(data.url, "https://prom.azure.com/api/v1/query_range");
                assert_eq!(data.method, "POST");
                assert_eq!(data.request_body, Some("query=up&start=1700000000&end=1700003600&step=60".to_string()));
                assert_eq!(data.status_code, Some(200));
                assert!(data.response_body.is_none()); // not sent by live capture
                assert_eq!(data.duration, Some(320));
                assert_eq!(data.request_headers.len(), 1);
                assert_eq!(data.response_headers.len(), 1);
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn parse_request_form_urlencoded_body() {
        // formData converted to URL-encoded by extension
        let json = r#"{"action":"request","data":{"url":"https://api.test/query","method":"POST","request_body":"query=max%20by%20(pod)%20(%0A%20%20up%0A)&start=1700000000&step=60","status_code":200}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Request { data } => {
                assert!(data.request_body.as_ref().unwrap().contains("query=max%20by"));
                assert!(data.request_body.as_ref().unwrap().contains("&start=1700000000"));
            }
            _ => panic!("Expected Request variant"),
        }
    }

    #[test]
    fn captured_request_serialization_roundtrip() {
        let req = CapturedRequest {
            url: "https://example.com/test".to_string(),
            method: "PUT".to_string(),
            request_headers: vec![
                HeaderPair { name: "Content-Type".to_string(), value: "application/json".to_string() },
                HeaderPair { name: "Authorization".to_string(), value: "Bearer abc".to_string() },
            ],
            request_body: Some(r#"{"key":"val"}"#.to_string()),
            status_code: Some(204),
            response_headers: vec![],
            response_body: None,
            duration: Some(88),
            timestamp: Some(1700000000.0),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: CapturedRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.url, req.url);
        assert_eq!(deserialized.method, req.method);
        assert_eq!(deserialized.request_headers.len(), 2);
        assert_eq!(deserialized.request_body, req.request_body);
        assert_eq!(deserialized.status_code, Some(204));
        assert!(deserialized.response_body.is_none());
        assert_eq!(deserialized.duration, Some(88));
    }

    #[test]
    fn serialize_ping_mode_messages() {
        let ping = OutgoingMessage::Ping;
        let json = serde_json::to_string(&ping).unwrap();
        assert!(json.contains(r#""action":"ping""#));

        let modes = ["off", "all", "filtered"];
        for mode in modes {
            let msg = OutgoingMessage::SetMode { mode: mode.to_string() };
            let json = serde_json::to_string(&msg).unwrap();
            assert!(json.contains(&format!(r#""mode":"{}""#, mode)));
        }
    }

    #[test]
    fn invalid_addr_returns_parse_error() {
        // Verify that an invalid SocketAddr string is properly rejected
        // by the same parse logic used in start_server
        let bad_addr = "not_a_valid_address:xyz";
        let result = bad_addr.parse::<std::net::SocketAddr>();
        assert!(
            result.is_err(),
            "invalid address should fail to parse as SocketAddr"
        );

        // Also verify a valid address parses correctly
        let good_addr = format!("127.0.0.1:{}", LIVE_CAPTURE_PORT);
        let result = good_addr.parse::<std::net::SocketAddr>();
        assert!(
            result.is_ok(),
            "valid address should parse successfully: {}",
            good_addr
        );
    }
}
