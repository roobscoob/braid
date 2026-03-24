use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;

use crate::models::{ContentBlock, Message};
use crate::settings::Settings;

/// Structured arguments for spawning a `claude` CLI process.
///
/// Every flag from `claude --help` is represented here. Fields that default to
/// `false` / `None` produce no CLI flag when left at their default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeArgs {
    // ── Prompt / Input ───────────────────────────────────────────────────

    /// The prompt to send (positional argument).
    pub prompt: String,

    /// Input format: "text" (default) or "stream-json".
    pub input_format: Option<InputFormat>,

    // ── Model / Cost ─────────────────────────────────────────────────────

    /// Model alias ("opus", "sonnet", "haiku") or full model name.
    pub model: Option<String>,

    /// Fallback model when default is overloaded.
    pub fallback_model: Option<String>,

    /// Effort level for the session.
    pub effort: Option<Effort>,

    /// Maximum conversation turns.
    pub max_turns: Option<u32>,

    /// Maximum spend in USD.
    pub max_budget_usd: Option<f64>,

    // ── System Prompt ────────────────────────────────────────────────────

    /// Custom system prompt (replaces the default).
    pub system_prompt: Option<String>,

    /// Text appended to the default system prompt.
    pub append_system_prompt: Option<String>,

    // ── Session Control ──────────────────────────────────────────────────

    /// Resume an existing session by ID.
    pub resume: Option<String>,

    /// Continue the most recent session.
    pub continue_session: bool,

    /// Use a specific session UUID.
    pub session_id: Option<String>,

    /// When resuming, fork into a new session instead of reusing the original.
    pub fork_session: bool,

    /// When resuming, only load messages up to this UUID.
    pub resume_session_at: Option<String>,

    /// Resume a session linked to a PR (by number or URL).
    pub from_pr: Option<String>,

    /// Display name for this session.
    pub name: Option<String>,

    /// Disable session persistence (sessions won't be saved to disk).
    pub no_session_persistence: bool,

    // ── Tools / Permissions ──────────────────────────────────────────────

    /// Override the built-in tool set. Use `""` to disable all, `"default"` for
    /// all, or specific names like `["Bash", "Edit", "Read"]`.
    pub tools: Option<Vec<String>>,

    /// Tool whitelist (e.g. `["Read", "Glob", "Grep"]`).
    pub allowed_tools: Option<Vec<String>>,

    /// Tool blacklist.
    pub disallowed_tools: Option<Vec<String>>,

    /// Permission mode override.
    pub permission_mode: Option<PermissionMode>,

    /// Enable `--dangerously-skip-permissions` as an *option* without
    /// activating it by default.
    pub allow_dangerously_skip_permissions: bool,

    /// Bypass all permission checks (only use in sandboxed environments).
    pub dangerously_skip_permissions: bool,

    /// Disable all skills / slash commands.
    pub disable_slash_commands: bool,

    // ── MCP / Plugins ────────────────────────────────────────────────────

    /// Paths (or JSON strings) for MCP server configs.
    pub mcp_config: Option<Vec<String>>,

    /// Only use MCP servers from `--mcp-config`, ignoring all other configs.
    pub strict_mcp_config: bool,

    /// Plugin directories to load for this session.
    pub plugin_dir: Option<Vec<PathBuf>>,

    // ── Agents ───────────────────────────────────────────────────────────

    /// Agent for the current session (overrides the 'agent' setting).
    pub agent: Option<String>,

    /// JSON object defining custom agents.
    pub agents: Option<String>,

    // ── Output / Streaming ───────────────────────────────────────────────

    /// Enable token-level streaming (`stream_event` messages).
    pub include_partial_messages: bool,

    /// Re-emit user messages from stdin back on stdout for acknowledgment.
    pub replay_user_messages: bool,

    /// Enable `SendUserMessage` tool for agent-to-user communication.
    pub brief: bool,

    /// JSON schema string for structured output validation.
    pub json_schema: Option<String>,

    // ── Directories / Files ──────────────────────────────────────────────

    /// Working directory for the CLI process.
    pub cwd: Option<PathBuf>,

    /// Additional directories to allow tool access to.
    pub add_dir: Option<Vec<PathBuf>>,

    /// File resources to download at startup (`file_id:relative_path`).
    pub file: Option<Vec<String>>,

    // ── Environment / Config ─────────────────────────────────────────────

    /// Path to a settings JSON file, a raw JSON string, or a typed [`Settings`] value.
    /// When a `Settings` struct is provided it is serialised to JSON and passed
    /// inline via `--settings`.
    pub settings: Option<SettingsArg>,

    /// Comma-separated list of setting sources (user, project, local).
    pub setting_sources: Option<String>,

    /// Beta headers for API requests.
    pub betas: Option<Vec<String>>,

    /// Minimal mode: skip hooks, LSP, plugin sync, etc.
    pub bare: bool,

    // ── IDE / Integrations ───────────────────────────────────────────────

    /// Auto-connect to IDE on startup.
    pub ide: bool,

    /// Enable Chrome integration.
    pub chrome: Option<bool>,

    // ── Worktree / Tmux ──────────────────────────────────────────────────

    /// Create a new git worktree for this session (optionally with a name).
    pub worktree: Option<Option<String>>,

    /// Create a tmux session for the worktree.
    pub tmux: Option<Option<String>>,

    // ── Debug ────────────────────────────────────────────────────────────

    /// Enable debug mode with optional category filter (e.g. "api,hooks").
    pub debug: Option<Option<String>>,

    /// Write debug logs to a specific file path.
    pub debug_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    BypassPermissions,
    Plan,
    DontAsk,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputFormat {
    Text,
    StreamJson,
}

/// How to pass settings to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingsArg {
    /// Path to a settings JSON file or a raw JSON string.
    Raw(String),
    /// A typed [`Settings`] value (serialized to JSON inline).
    Typed(Settings),
}

/// Errors from the stream — either a parse failure or a CLI-level error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamError {
    /// A stdout line that didn't parse as a known [`Message`].
    ParseError { raw: String, reason: String },
    /// One or more lines written to stderr by the CLI.
    CliError { message: String, exit_code: Option<i32> },
}

/// Error type for `spawn_claude`.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("failed to spawn claude process: {0}")]
    Spawn(#[from] std::io::Error),
}

/// A single item yielded by the stream returned from [`spawn_claude`].
#[derive(Debug, Clone)]
pub enum StreamItem {
    /// A successfully parsed protocol message.
    Message(Message),
    /// An error from the CLI or a parse failure.
    Error(StreamError),
}

// ─── Stdin Messages (written to claude's stdin) ──────────────────────────────

/// A message written to the CLI's stdin when using `--input-format stream-json`.
///
/// The format mirrors the output `user` message envelope.  This is the same
/// structure the official TypeScript SDK sends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputMessage {
    #[serde(rename = "type")]
    pub kind: String, // always "user"
    pub session_id: String,
    pub message: UserInputBody,
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputBody {
    pub role: String, // always "user"
    pub content: Vec<ContentBlock>,
}

impl UserInputMessage {
    /// Create a simple text message to send to the CLI.
    pub fn text(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: "user".into(),
            session_id: session_id.into(),
            message: UserInputBody {
                role: "user".into(),
                content: vec![ContentBlock::Text { text: text.into() }],
            },
            parent_tool_use_id: None,
        }
    }
}

// ─── Claude Session ──────────────────────────────────────────────────────────

/// Handle to a running `claude` CLI process.
///
/// Provides both an output stream (stdout parsed into [`StreamItem`]s) and
/// a sender for writing messages to the process's stdin.
pub struct ClaudeSession {
    /// Stream of parsed output messages.
    pub stream: ReceiverStream<StreamItem>,

    /// Write handle to the CLI's stdin.  Send [`UserInputMessage`]s here to
    /// feed context or follow-up prompts into the conversation.
    ///
    /// Dropping the sender closes stdin, which signals the CLI that no more
    /// input is coming (it will finish the current turn and exit).
    stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
}

impl ClaudeSession {
    /// Send a typed message to the CLI's stdin.
    pub async fn send(&self, msg: &UserInputMessage) -> Result<(), std::io::Error> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin closed"))?;
        let mut line = serde_json::to_string(msg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Send a plain text message.
    pub async fn send_text(
        &self,
        session_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), std::io::Error> {
        self.send(&UserInputMessage::text(session_id, text)).await
    }

    /// Close stdin, signalling the CLI that no more input is coming.
    /// The CLI will finish its current turn and exit.
    pub async fn close_stdin(&self) {
        let mut guard = self.stdin.lock().await;
        *guard = None;
    }
}

/// Spawn a `claude` CLI process and return a [`ClaudeSession`].
///
/// The session provides a stream of [`StreamItem`]s (stdout) and a sender
/// for writing [`UserInputMessage`]s (stdin).
pub fn spawn_claude(args: ClaudeArgs) -> Result<ClaudeSession, SpawnError> {
    let mut cmd = Command::new("claude");

    // On Windows, prevent the subprocess from opening a visible console window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // Required flags for structured streaming output.
    cmd.args(["--print", "--output-format", "stream-json", "--verbose"]);

    // ── Model / Cost ─────────────────────────────────────────────────────
    if let Some(ref model) = args.model {
        cmd.args(["--model", model]);
    }
    if let Some(ref fallback) = args.fallback_model {
        cmd.args(["--fallback-model", fallback]);
    }
    if let Some(ref effort) = args.effort {
        cmd.args(["--effort", match effort {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Max => "max",
        }]);
    }
    if let Some(turns) = args.max_turns {
        cmd.args(["--max-turns", &turns.to_string()]);
    }
    if let Some(budget) = args.max_budget_usd {
        cmd.args(["--max-budget-usd", &budget.to_string()]);
    }

    // ── System Prompt ────────────────────────────────────────────────────
    if let Some(ref sp) = args.system_prompt {
        cmd.args(["--system-prompt", sp]);
    }
    if let Some(ref asp) = args.append_system_prompt {
        cmd.args(["--append-system-prompt", asp]);
    }

    // ── Session Control ──────────────────────────────────────────────────
    if let Some(ref session_id) = args.resume {
        cmd.args(["--resume", session_id]);
    }
    if args.continue_session {
        cmd.arg("--continue");
    }
    if let Some(ref sid) = args.session_id {
        cmd.args(["--session-id", sid]);
    }
    if args.fork_session {
        cmd.arg("--fork-session");
    }
    if let Some(ref at) = args.resume_session_at {
        cmd.args(["--resume-session-at", at]);
    }
    if let Some(ref pr) = args.from_pr {
        cmd.args(["--from-pr", pr]);
    }
    if let Some(ref name) = args.name {
        cmd.args(["--name", name]);
    }
    if args.no_session_persistence {
        cmd.arg("--no-session-persistence");
    }

    // ── Tools / Permissions ──────────────────────────────────────────────
    if let Some(ref tools) = args.tools {
        cmd.args(["--tools", &tools.join(",")]);
    }
    if let Some(ref tools) = args.allowed_tools {
        cmd.args(["--allowed-tools", &tools.join(",")]);
    }
    if let Some(ref tools) = args.disallowed_tools {
        cmd.args(["--disallowed-tools", &tools.join(",")]);
    }
    if let Some(ref mode) = args.permission_mode {
        cmd.args(["--permission-mode", match mode {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::Plan => "plan",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Auto => "auto",
        }]);
    }
    if args.allow_dangerously_skip_permissions {
        cmd.arg("--allow-dangerously-skip-permissions");
    }
    if args.dangerously_skip_permissions {
        cmd.arg("--dangerously-skip-permissions");
    }
    if args.disable_slash_commands {
        cmd.arg("--disable-slash-commands");
    }

    // ── MCP / Plugins ────────────────────────────────────────────────────
    if let Some(ref configs) = args.mcp_config {
        cmd.arg("--mcp-config");
        for config in configs {
            cmd.arg(config);
        }
    }
    if args.strict_mcp_config {
        cmd.arg("--strict-mcp-config");
    }
    if let Some(ref dirs) = args.plugin_dir {
        for dir in dirs {
            cmd.args(["--plugin-dir", &dir.to_string_lossy()]);
        }
    }

    // ── Agents ───────────────────────────────────────────────────────────
    if let Some(ref agent) = args.agent {
        cmd.args(["--agent", agent]);
    }
    if let Some(ref agents_json) = args.agents {
        cmd.args(["--agents", agents_json]);
    }

    // ── Output / Streaming ───────────────────────────────────────────────
    if let Some(ref fmt) = args.input_format {
        cmd.args(["--input-format", match fmt {
            InputFormat::Text => "text",
            InputFormat::StreamJson => "stream-json",
        }]);
    }
    if args.include_partial_messages {
        cmd.arg("--include-partial-messages");
    }
    if args.replay_user_messages {
        cmd.arg("--replay-user-messages");
    }
    if args.brief {
        cmd.arg("--brief");
    }
    if let Some(ref schema) = args.json_schema {
        cmd.args(["--json-schema", schema]);
    }

    // ── Directories / Files ──────────────────────────────────────────────
    if let Some(ref dirs) = args.add_dir {
        cmd.arg("--add-dir");
        for dir in dirs {
            cmd.arg(&dir.to_string_lossy().into_owned());
        }
    }
    if let Some(ref files) = args.file {
        cmd.arg("--file");
        for f in files {
            cmd.arg(f);
        }
    }

    // ── Environment / Config ─────────────────────────────────────────────
    if let Some(ref s) = args.settings {
        let value = match s {
            SettingsArg::Raw(raw) => raw.clone(),
            SettingsArg::Typed(settings) => serde_json::to_string(settings)
                .expect("Settings should always be serializable"),
        };
        cmd.args(["--settings", &value]);
    }
    if let Some(ref sources) = args.setting_sources {
        cmd.args(["--setting-sources", sources]);
    }
    if let Some(ref betas) = args.betas {
        cmd.arg("--betas");
        for b in betas {
            cmd.arg(b);
        }
    }
    if args.bare {
        cmd.arg("--bare");
    }

    // ── IDE / Integrations ───────────────────────────────────────────────
    if args.ide {
        cmd.arg("--ide");
    }
    match args.chrome {
        Some(true) => { cmd.arg("--chrome"); }
        Some(false) => { cmd.arg("--no-chrome"); }
        None => {}
    }

    // ── Worktree / Tmux ──────────────────────────────────────────────────
    if let Some(ref wt) = args.worktree {
        match wt {
            Some(name) => { cmd.args(["--worktree", name]); }
            None => { cmd.arg("--worktree"); }
        }
    }
    if let Some(ref tmux) = args.tmux {
        match tmux {
            Some(style) => { cmd.args(["--tmux", style]); }
            None => { cmd.arg("--tmux"); }
        }
    }

    // ── Debug ────────────────────────────────────────────────────────────
    if let Some(ref dbg) = args.debug {
        match dbg {
            Some(filter) => { cmd.args(["--debug", filter]); }
            None => { cmd.arg("--debug"); }
        }
    }
    if let Some(ref path) = args.debug_file {
        cmd.args(["--debug-file", &path.to_string_lossy()]);
    }

    // ── Separator + Prompt ───────────────────────────────────────────────
    cmd.arg("--");
    cmd.arg(&args.prompt);

    // Working directory.
    if let Some(ref cwd) = args.cwd {
        cmd.current_dir(cwd);
    }

    // Only pipe stdin when an input format is set (caller intends to write).
    // Otherwise use null so the CLI doesn't wait for data that never comes.
    if args.input_format.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let stdin_handle: Arc<Mutex<Option<tokio::process::ChildStdin>>> =
        Arc::new(Mutex::new(child.stdin.take()));

    let (tx, rx) = mpsc::channel::<StreamItem>(64);

    // Collect stderr lines in the background.
    let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_lines_writer = Arc::clone(&stderr_lines);
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            stderr_lines_writer.lock().await.push(line);
        }
    });

    // Background task: read stdout, parse messages, then check exit status.
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() {
                continue;
            }

            let item = match serde_json::from_str::<Message>(&line) {
                Ok(msg) => StreamItem::Message(msg),
                Err(e) => StreamItem::Error(StreamError::ParseError { raw: line, reason: e.to_string() }),
            };

            if tx.send(item).await.is_err() {
                break; // receiver dropped
            }
        }

        // Wait for the process to finish and surface any errors.
        let status = child.wait().await;
        let exit_code = status.ok().and_then(|s| s.code());
        let stderr_content = stderr_lines.lock().await;
        let stderr_msg = stderr_content.join("\n");

        // Only emit a CLI error if stderr has content.  A non-zero exit
        // with empty stderr is already covered by the Result message.
        if !stderr_msg.is_empty() {
            let _ = tx
                .send(StreamItem::Error(StreamError::CliError {
                    message: stderr_msg,
                    exit_code,
                }))
                .await;
        }
    });

    Ok(ClaudeSession {
        stream: ReceiverStream::new(rx),
        stdin: stdin_handle,
    })
}
