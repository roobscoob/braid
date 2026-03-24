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

/// Quick check if a string looks like a UUID (8-4-4-4-12 hex with dashes).
fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes()[8] == b'-'
        && s.as_bytes()[13] == b'-'
        && s.as_bytes()[18] == b'-'
        && s.as_bytes()[23] == b'-'
}

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
    /// node_id → jail mount UUID. Used to reject tool calls that reference
    /// a stale jail path (e.g. after branching gives Claude a new jail).
    jail_ids: Mutex<HashMap<String, String>>,
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

    /// Register the jail UUID for a given node (turn). Called when a turn
    /// starts so the hook server knows which jail path is valid.
    pub async fn set_jail_id(&self, node_id: String, jail_id: String) {
        self.jail_ids.lock().await.insert(node_id, jail_id);
    }

    /// Check if a tool input contains a jail UUID that doesn't match
    /// the current turn's jail. Returns the correct UUID if mismatched.
    async fn check_stale_jail(&self, node_id: Option<&str>, tool_input: &serde_json::Value) -> Option<String> {
        let node_id = node_id?;
        let current_uuid = self.jail_ids.lock().await.get(node_id)?.clone();

        // Serialize tool_input to string and search for any jail UUID pattern.
        let input_str = tool_input.to_string();
        let jail_marker = "braid/jails/";
        // Could also be braid\\jails\\ in Windows paths
        let jail_marker_win = "braid\\\\jails\\\\";

        for marker in [jail_marker, jail_marker_win] {
            if let Some(pos) = input_str.find(marker) {
                let after = &input_str[pos + marker.len()..];
                // Extract the UUID (36 chars: 8-4-4-4-12)
                if after.len() >= 36 {
                    let found_uuid = &after[..36];
                    if found_uuid != current_uuid && looks_like_uuid(found_uuid) {
                        return Some(current_uuid);
                    }
                }
            }
        }

        None
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
        jail_ids: Mutex::new(HashMap::new()),
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

                let tool_name = request
                    .input
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                eprintln!(
                    "[hook-server] request: event={} node={:?} tool={tool_name}",
                    request.event,
                    request.node_id,
                );

                // Reject tool calls that reference a stale jail path.
                // This happens when Claude resumes a session after branching
                // and uses paths from the old jail in the transcript history.
                let tool_input = request.input.get("tool_input").cloned()
                    .unwrap_or(serde_json::Value::Null);
                if let Some(correct_uuid) = state.check_stale_jail(
                    request.node_id.as_deref(),
                    &tool_input,
                ).await {
                    let reason = format!(
                        "Typo in path — your current working directory is in jail {}. \
                         Please use the correct path.",
                        correct_uuid,
                    );
                    eprintln!("[hook-server] rejected stale jail path for {tool_name}: {reason}");
                    let resp = deny_response(&reason);
                    let resp_line = format!("{}\n", resp.trim());
                    let _ = writer.write_all(resp_line.as_bytes()).await;
                    return;
                }

                // Auto-approve safe tools without bothering the frontend.
                const AUTO_ALLOW: &[&str] = &[
                    "Read", "Write", "Edit", "Agent", "Glob", "Grep", "Task",
                ];
                if AUTO_ALLOW.iter().any(|&t| t == tool_name) {
                    eprintln!("[hook-server] auto-approved: {tool_name}");
                    let _ = writer.write_all(b"{}\n").await;
                    return;
                }

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
                    tool_name: Some(tool_name.to_string()).filter(|s| !s.is_empty()),
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
