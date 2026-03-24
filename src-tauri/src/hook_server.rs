//! IPC server for hook communication.
//!
//! The main Tauri process runs a TCP listener. When Claude invokes a hook,
//! the hook subprocess connects to this server, sends the hook payload,
//! and waits for a decision from the UI.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};

/// Build a PreToolUse deny response JSON string.
fn deny_response(reason: &str) -> String {
    let output = crate::hooks::HookOutput::deny(reason);
    serde_json::to_string(&output).unwrap()
}

/// A pending hook request awaiting a decision from the frontend.
struct PendingHook {
    tx: oneshot::Sender<String>,
}

/// Shared state for the hook server.
pub struct HookServerState {
    /// Port the server is listening on.
    pub port: u16,
    /// Pending requests keyed by request ID.
    pending: Mutex<HashMap<String, PendingHook>>,
}

/// The payload sent from the hook subprocess to the main process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRequest {
    /// Unique ID for this request (for correlating the response).
    pub request_id: String,
    /// The conversation node this hook belongs to.
    pub node_id: Option<String>,
    /// The hook event name.
    pub event: String,
    /// The raw hook input JSON from Claude.
    pub input: serde_json::Value,
}

/// Event emitted to the frontend when a hook needs a decision.
#[derive(Debug, Clone, Serialize)]
pub struct HookDecisionRequest {
    pub request_id: String,
    pub node_id: Option<String>,
    pub event: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
}

impl HookServerState {
    /// Resolve a pending hook request with the given JSON response.
    pub async fn resolve(&self, request_id: &str, response_json: String) {
        let mut pending = self.pending.lock().await;
        if let Some(hook) = pending.remove(request_id) {
            let _ = hook.tx.send(response_json);
        }
    }
}

/// Start the hook IPC server. Returns the shared state (which includes the port).
pub async fn start_hook_server(app: AppHandle) -> Arc<HookServerState> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind hook server");
    let port = listener.local_addr().unwrap().port();

    let state = Arc::new(HookServerState {
        port,
        pending: Mutex::new(HashMap::new()),
    });

    eprintln!("[hook-server] listening on 127.0.0.1:{port}");

    let state_clone = state.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _addr): (tokio::net::TcpStream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    eprintln!("[hook-server] accept error: {e}");
                    continue;
                }
            };

            let state = state_clone.clone();
            let app = app.clone();

            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);
                let mut line = String::new();

                // Read the hook request (single JSON line).
                if let Err(e) = reader.read_line(&mut line).await {
                    eprintln!("[hook-server] read error: {e}");
                    return;
                }

                let request: HookRequest = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[hook-server] parse error: {e}");
                        // Return a default allow response.
                        let _: Result<(), _> = writer
                            .write_all(b"{}\n")
                            .await;
                        return;
                    }
                };

                eprintln!(
                    "[hook-server] request: event={} node={:?} tool={:?}",
                    request.event,
                    request.node_id,
                    request.input.get("tool_name").and_then(|v| v.as_str()),
                );

                // Create a oneshot channel to wait for the frontend's decision.
                let (tx, rx) = oneshot::channel();
                let request_id = request.request_id.clone();

                {
                    let mut pending = state.pending.lock().await;
                    pending.insert(request_id.clone(), PendingHook { tx });
                }

                // Emit event to the frontend.
                let decision_request = HookDecisionRequest {
                    request_id: request_id.clone(),
                    node_id: request.node_id.clone(),
                    event: request.event.clone(),
                    tool_name: request
                        .input
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    tool_input: request
                        .input
                        .get("tool_input")
                        .cloned(),
                };

                let _ = app.emit("hook-decision-request", &decision_request);

                // Wait for the frontend to respond. No timeout — the user
                // might leave their machine and come back later.
                let response = match rx.await {
                    Ok(json) => json,
                    Err(_) => {
                        eprintln!("[hook-server] channel dropped for {request_id}, denying");
                        deny_response("hook decision channel dropped")
                    }
                };

                // Send the response back to the hook subprocess.
                let response_line = format!("{}\n", response.trim());
                if let Err(e) = writer.write_all(response_line.as_bytes()).await {
                    eprintln!("[hook-server] write error: {e}");
                }
            });
        }
    });

    state
}
