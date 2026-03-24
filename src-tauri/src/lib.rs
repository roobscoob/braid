pub mod claude;
pub mod conversation;
pub mod hook_server;
pub mod hooks;
pub mod jail;
pub mod models;
pub mod settings;

use claude::ClaudeArgs;
use conversation::ContextToken;
use futures::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use crate::claude::{StreamError, StreamItem};
use crate::models::{ContentBlock, Delta, Message, StreamEvent};

/// Shared VCS stores, keyed by project path.
struct VcsStores {
    stores: Mutex<HashMap<std::path::PathBuf, Arc<jail::vcs::VcsStore>>>,
}

impl VcsStores {
    fn new() -> Self {
        Self {
            stores: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create a VcsStore for a project path.
    fn get_or_create(&self, project_path: &std::path::Path) -> Result<Arc<jail::vcs::VcsStore>, String> {
        let canonical = std::fs::canonicalize(project_path)
            .unwrap_or_else(|_| project_path.to_path_buf());
        let mut stores = self.stores.lock().unwrap();
        if let Some(store) = stores.get(&canonical) {
            Ok(store.clone())
        } else {
            let store = jail::vcs::VcsStore::open(&canonical)
                .map_err(|e| format!("VCS init failed: {e}"))?;
            stores.insert(canonical, store.clone());
            Ok(store)
        }
    }
}

/// A single event pushed to the frontend during a turn.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TurnEvent {
    Started {
        turn_id: String,
    },
    SystemInit {
        turn_id: String,
        session_id: String,
        model: String,
    },
    TextDelta {
        turn_id: String,
        text: String,
        /// If set, this text belongs to a subagent spawned by this tool_use_id.
        parent_tool_use_id: Option<String>,
    },
    ThinkingDelta {
        turn_id: String,
        text: String,
        parent_tool_use_id: Option<String>,
    },
    ToolUseStart {
        turn_id: String,
        tool_name: String,
        tool_id: String,
        parent_tool_use_id: Option<String>,
    },
    ToolUseInputDelta {
        turn_id: String,
        tool_id: String,
        partial_json: String,
    },
    ToolResult {
        turn_id: String,
        tool_id: String,
        content: String,
        is_error: bool,
        parent_tool_use_id: Option<String>,
    },
    Finished {
        turn_id: String,
        session_id: String,
        message_id: String,
        model: String,
        cost_usd: f64,
        duration_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        num_turns: u32,
        is_error: bool,
        result_text: String,
        commit_sha: Option<String>,
    },
    /// A background agent completed a tool call (from task_progress).
    AgentProgress {
        turn_id: String,
        /// The Agent tool_use_id this progress belongs to.
        agent_tool_id: String,
        tool_name: String,
        description: String,
    },
    /// Claude resumed generating after a background task completed.
    Resume {
        turn_id: String,
    },
    /// Jail commit started.
    Committing {
        turn_id: String,
    },
    /// Jail commit finished.
    Committed {
        turn_id: String,
        commit_sha: String,
        file_count: usize,
    },
    Error {
        turn_id: String,
        message: String,
    },
}

fn emit(app: &AppHandle, event: TurnEvent) {
    let _ = app.emit("turn-event", &event);
}

/// Run a streaming turn, emitting events to the frontend as they arrive.
/// If `jail` is provided, Claude runs inside it and changes are committed after.
async fn stream_turn(
    app: AppHandle,
    turn_id: String,
    args: ClaudeArgs,
    jail: Option<jail::Jail>,
    parent_commit: Option<String>,
) {
    let session = match crate::claude::spawn_claude(args) {
        Ok(s) => s,
        Err(e) => {
            emit(
                &app,
                TurnEvent::Error {
                    turn_id,
                    message: e.to_string(),
                },
            );
            return;
        }
    };

    let mut stream = session.stream;
    let mut session_id = String::new();
    let mut last_assistant_uuid = String::new();
    let mut model = String::new();
    let mut current_tool_id = String::new(); // track which tool_use is currently streaming
    let mut seen_first_init = false;
    // Track background agent tool_use_ids — these don't get assistant
    // messages with parent_tool_use_id, so we use task_progress instead.
    let mut bg_agent_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    while let Some(item) = stream.next().await {
        match item {
            StreamItem::Message(msg) => match msg {
                Message::System(sys) => {
                    match sys.subtype.as_str() {
                        "init" => {
                            session_id = sys.session_id.clone();
                            model = sys.model.clone().unwrap_or_default();
                            if !seen_first_init {
                                seen_first_init = true;
                                emit(
                                    &app,
                                    TurnEvent::SystemInit {
                                        turn_id: turn_id.clone(),
                                        session_id: session_id.clone(),
                                        model: model.clone(),
                                    },
                                );
                            } else {
                                // Subsequent init = Claude resumed after a
                                // background task. Split the text stream.
                                emit(
                                    &app,
                                    TurnEvent::Resume {
                                        turn_id: turn_id.clone(),
                                    },
                                );
                            }
                        }
                        "task_progress" => {
                            // For background agents only: emit tool calls
                            // from task_progress since we don't get assistant
                            // messages with parent_tool_use_id for them.
                            if let Some(agent_id) = &sys.tool_use_id {
                                if bg_agent_ids.contains(agent_id) {
                                    if let Some(tool_name) = &sys.last_tool_name {
                                        let description =
                                            sys.description.as_deref().unwrap_or("").to_string();
                                        emit(
                                            &app,
                                            TurnEvent::AgentProgress {
                                                turn_id: turn_id.clone(),
                                                agent_tool_id: agent_id.clone(),
                                                tool_name: tool_name.clone(),
                                                description,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        "task_completed" | "task_notification" => {
                            // For background agents: mark remaining tool
                            // calls as done.
                            if let Some(agent_id) = &sys.tool_use_id {
                                if bg_agent_ids.contains(agent_id) {
                                    // We don't track a queue anymore — the
                                    // synthetic IDs from task_progress don't
                                    // have matching results. Just emit a
                                    // completion marker for the last one.
                                    // The frontend will see the agent block
                                    // transition via the Resume event.
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Message::Assistant(asst) => {
                    last_assistant_uuid = asst.uuid.clone();
                    if model.is_empty() {
                        model = asst.message.model.clone();
                    }
                    // Detect background agent spawns from top-level messages.
                    if asst.parent_tool_use_id.is_none() {
                        for block in &asst.message.content {
                            if let ContentBlock::ToolUse { id, name, input } = block {
                                if name == "Agent" {
                                    if input
                                        .get("run_in_background")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                    {
                                        bg_agent_ids.insert(id.clone());
                                    }
                                }
                            }
                        }
                    }
                    let parent = asst.parent_tool_use_id.clone();
                    if parent.is_some() {
                        // Subagent assistant message — emit tool calls
                        // from its content blocks (stream events don't
                        // flow for subagents, only full messages).
                        for block in &asst.message.content {
                            if let ContentBlock::ToolUse { id, name, input } = block {
                                emit(
                                    &app,
                                    TurnEvent::ToolUseStart {
                                        turn_id: turn_id.clone(),
                                        tool_name: name.clone(),
                                        tool_id: id.clone(),
                                        parent_tool_use_id: parent.clone(),
                                    },
                                );
                                let json = serde_json::to_string_pretty(input).unwrap_or_default();
                                if !json.is_empty() {
                                    emit(
                                        &app,
                                        TurnEvent::ToolUseInputDelta {
                                            turn_id: turn_id.clone(),
                                            tool_id: id.clone(),
                                            partial_json: json,
                                        },
                                    );
                                }
                            }
                        }
                    }
                    // Top-level assistant messages: content blocks are
                    // already emitted via StreamEvent deltas.
                }
                Message::User(user) => {
                    let parent = user.parent_tool_use_id.clone();
                    for block in &user.message.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            let text = match content {
                                serde_json::Value::String(s) => s.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            };
                            emit(
                                &app,
                                TurnEvent::ToolResult {
                                    turn_id: turn_id.clone(),
                                    tool_id: tool_use_id.clone(),
                                    content: text,
                                    is_error: is_error.unwrap_or(false),
                                    parent_tool_use_id: parent.clone(),
                                },
                            );
                        }
                    }
                }
                Message::StreamEvent(se) => {
                    let parent = se.parent_tool_use_id.clone();
                    match se.event {
                        StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                            Delta::TextDelta { text } => {
                                emit(
                                    &app,
                                    TurnEvent::TextDelta {
                                        turn_id: turn_id.clone(),
                                        text,
                                        parent_tool_use_id: parent,
                                    },
                                );
                            }
                            Delta::ThinkingDelta { thinking } => {
                                emit(
                                    &app,
                                    TurnEvent::ThinkingDelta {
                                        turn_id: turn_id.clone(),
                                        text: thinking,
                                        parent_tool_use_id: parent,
                                    },
                                );
                            }
                            Delta::SignatureDelta { .. } => {}
                            Delta::InputJsonDelta { partial_json } => {
                                emit(
                                    &app,
                                    TurnEvent::ToolUseInputDelta {
                                        turn_id: turn_id.clone(),
                                        tool_id: current_tool_id.clone(),
                                        partial_json,
                                    },
                                );
                            }
                        },
                        StreamEvent::ContentBlockStart {
                            content_block: ContentBlock::ToolUse { id, name, .. },
                            ..
                        } => {
                            current_tool_id = id.clone();
                            emit(
                                &app,
                                TurnEvent::ToolUseStart {
                                    turn_id: turn_id.clone(),
                                    tool_name: name,
                                    tool_id: id,
                                    parent_tool_use_id: parent,
                                },
                            );
                        }
                        _ => {}
                    }
                }
                Message::Result(res) => {
                    let usage = res.usage.as_ref();
                    let sid = if session_id.is_empty() {
                        res.session_id.clone()
                    } else {
                        session_id.clone()
                    };
                    let mid = if last_assistant_uuid.is_empty() {
                        res.uuid.clone()
                    } else {
                        last_assistant_uuid.clone()
                    };
                    // Commit jail changes if active.
                    let commit_sha = if let Some(ref jail) = jail {
                        emit(&app, TurnEvent::Committing { turn_id: turn_id.clone() });
                        let parent_oid = parent_commit
                            .as_ref()
                            .and_then(|s| gix::ObjectId::from_hex(s.as_bytes()).ok())
                            .or_else(|| jail.vcs.head_commit_id().ok());
                        match parent_oid {
                            Some(oid) => {
                                match jail.commit(&format!("turn:{}", turn_id), oid) {
                                    Ok(commit_id) => {
                                        let sha = commit_id.to_string();
                                        let file_count = jail.tracker.mutations().len();
                                        emit(&app, TurnEvent::Committed {
                                            turn_id: turn_id.clone(),
                                            commit_sha: sha.clone(),
                                            file_count,
                                        });
                                        Some(sha)
                                    }
                                    Err(e) => {
                                        emit(
                                            &app,
                                            TurnEvent::Error {
                                                turn_id: turn_id.clone(),
                                                message: format!("Jail commit failed: {e}"),
                                            },
                                        );
                                        None
                                    }
                                }
                            }
                            None => {
                                emit(
                                    &app,
                                    TurnEvent::Error {
                                        turn_id: turn_id.clone(),
                                        message: "No parent commit available".into(),
                                    },
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };

                    emit(
                        &app,
                        TurnEvent::Finished {
                            turn_id: turn_id.clone(),
                            session_id: sid,
                            message_id: mid,
                            model: model.clone(),
                            cost_usd: res.total_cost_usd,
                            duration_ms: res.duration_ms,
                            input_tokens: usage.map_or(0, |u| u.input_tokens),
                            output_tokens: usage.map_or(0, |u| u.output_tokens),
                            num_turns: res.num_turns,
                            is_error: res.is_error,
                            result_text: res.result.clone(),
                            commit_sha,
                        },
                    );
                }
                _ => {}
            },
            StreamItem::Error(e) => match e {
                StreamError::CliError { message, exit_code } => {
                    let msg = format!("CLI error (exit {:?}): {}", exit_code, message);
                    eprintln!("[stream] {msg}");
                    emit(
                        &app,
                        TurnEvent::Error {
                            turn_id: turn_id.clone(),
                            message: msg,
                        },
                    );
                }
                StreamError::ParseError { raw, reason } => {
                    eprintln!("[stream] parse error: {reason}");
                    eprintln!("[stream]   raw ({} bytes): {raw}", raw.len());
                }
            },
        }
    }

    // If we never emitted a Finished event, make sure the frontend knows the turn ended.
    if session_id.is_empty() && last_assistant_uuid.is_empty() {
        emit(
            &app,
            TurnEvent::Error {
                turn_id,
                message: "Turn ended without any response from Claude".into(),
            },
        );
    }
}

/// Shared defaults for an isolated Claude instance that still uses OAuth.
/// `node_id` is embedded in the PreToolUse hook command so the hook handler
/// knows which conversation node the tool call belongs to.
fn base_args(node_id: &str, hook_port: u16) -> ClaudeArgs {
    use crate::settings::*;

    // Build the hook command pointing at our own executable.
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("braid"))
        .to_string_lossy()
        .to_string();

    let hook_command =
        format!("\"{exe}\" --hook pre_tool_use --node {node_id} --hook-port {hook_port}");

    let settings = Settings {
        hooks: Some(Hooks {
            pre_tool_use: Some(vec![HookMatcher {
                matcher: Some("*".into()),
                hooks: vec![HookCommand::Command {
                    command: hook_command,
                    timeout: None,
                    is_async: None,
                    status_message: None,
                }],
            }]),
            ..Default::default()
        }),
        permissions: Some(Permissions {
            default_mode: Some(DefaultPermissionMode::BypassPermissions),
            ..Default::default()
        }),
        ..Default::default()
    };

    ClaudeArgs {
        include_partial_messages: true,
        strict_mcp_config: true,
        disable_slash_commands: true,
        chrome: Some(false),
        setting_sources: Some("".into()),
        settings: Some(crate::claude::SettingsArg::Typed(settings)),
        allow_dangerously_skip_permissions: true,
        ..Default::default()
    }
}

#[tauri::command]
async fn start_conversation(
    app: AppHandle,
    state: tauri::State<'_, Arc<hook_server::HookServerState>>,
    vcs_stores: tauri::State<'_, VcsStores>,
    prompt: String,
    project_path: Option<String>,
) -> Result<String, String> {
    let turn_id = uuid::Uuid::new_v4().to_string();

    // Create a jail if a project path is provided.
    let jail = if let Some(ref pp) = project_path {
        let pp = std::path::PathBuf::from(pp);
        let vcs = vcs_stores.get_or_create(&pp)?;
        Some(
            jail::Jail::create(jail::JailConfig {
                project_path: pp,
                jail_base: None,
                vcs,
            })
            .map_err(|e| format!("Failed to create jail: {e}"))?,
        )
    } else {
        None
    };

    let mut args = ClaudeArgs {
        prompt: prompt.clone(),
        ..base_args(&turn_id, state.port)
    };

    // Point Claude at the jail's working directory.
    if let Some(ref j) = jail {
        args.cwd = Some(j.root.clone());
    }

    emit(
        &app,
        TurnEvent::Started {
            turn_id: turn_id.clone(),
        },
    );

    let tid = turn_id.clone();
    tauri::async_runtime::spawn(async move {
        stream_turn(app, tid, args, jail, None).await;
    });

    Ok(turn_id)
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: tauri::State<'_, Arc<hook_server::HookServerState>>,
    vcs_stores: tauri::State<'_, VcsStores>,
    session_id: String,
    message_id: String,
    prompt: String,
    project_path: Option<String>,
    commit_sha: Option<String>,
) -> Result<String, String> {
    let turn_id = uuid::Uuid::new_v4().to_string();

    // Create a jail if a project path is provided.
    let jail = if let Some(ref pp) = project_path {
        let pp = std::path::PathBuf::from(pp);
        let vcs = vcs_stores.get_or_create(&pp)?;
        Some(
            jail::Jail::create(jail::JailConfig {
                project_path: pp,
                jail_base: None,
                vcs,
            })
            .map_err(|e| format!("Failed to create jail: {e}"))?,
        )
    } else {
        None
    };

    let mut args = ClaudeArgs {
        prompt: prompt.clone(),
        no_session_persistence: false,
        resume: Some(session_id),
        resume_session_at: Some(message_id),
        fork_session: true,
        ..base_args(&turn_id, state.port)
    };

    if let Some(ref j) = jail {
        args.cwd = Some(j.root.clone());
    }

    emit(
        &app,
        TurnEvent::Started {
            turn_id: turn_id.clone(),
        },
    );

    let tid = turn_id.clone();
    tauri::async_runtime::spawn(async move {
        stream_turn(app, tid, args, jail, commit_sha).await;
    });

    Ok(turn_id)
}

/// Resolve a pending hook decision from the frontend.
#[tauri::command]
async fn resolve_hook(
    state: tauri::State<'_, std::sync::Arc<hook_server::HookServerState>>,
    request_id: String,
    response_json: String,
) -> Result<(), String> {
    state.resolve(&request_id, response_json).await;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // Start the hook IPC server.
            let state = tauri::async_runtime::block_on(hook_server::start_hook_server(handle));
            app.manage(state);
            app.manage(VcsStores::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_conversation,
            send_message,
            resolve_hook,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
