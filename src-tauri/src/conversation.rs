//! Token-based conversation API for tree-structured interactions.
//!
//! A [`ContextToken`] is a pointer to a node in a conversation tree.  Calling
//! [`ContextToken::message`] forks from that node, producing a new child node
//! with its own token.  This gives you free branching — call `.message()` on
//! the same token multiple times to create siblings.
//!
//! ```ignore
//! let root = ContextToken::start("hello", args).await?;
//! let a = root.token.message("option A", args).await?;
//! let b = root.token.message("option B", args).await?;  // sibling of a
//! let c = a.token.message("follow up", args).await?;     // child of a
//! ```

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::claude::{ClaudeArgs, SpawnError, StreamItem, spawn_claude};
use crate::models::{ContentBlock, Message, ResultMessage};

// ─── Public Types ────────────────────────────────────────────────────────────

/// A pointer to a specific node in a conversation tree.
///
/// Cheap to clone.  Two tokens with the same `session_id` + `message_id`
/// point to the same node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ContextToken {
    /// The CLI session that owns this node.
    pub session_id: String,
    /// UUID of the assistant message at this node.
    pub message_id: String,
    /// Git commit SHA of the code state at this node (if jailed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
}

/// The result of a single turn (one user prompt → one assistant response).
#[derive(Debug, Clone)]
pub struct Turn {
    /// The assistant's text response.
    pub text: String,
    /// Token pointing to this node — use it to continue or branch.
    pub token: ContextToken,
    /// Model that generated the response.
    pub model: String,
    /// Total cost of this turn in USD.
    pub cost_usd: f64,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Tokens consumed.
    pub input_tokens: u64,
    /// Tokens produced.
    pub output_tokens: u64,
    /// Number of agentic turns (tool calls) within this response.
    pub num_turns: u32,
    /// Whether the result was flagged as an error by the CLI.
    pub is_error: bool,
    /// The full result message from the CLI.
    pub result_message: Option<ResultMessage>,
}

/// Errors from a conversation turn.
#[derive(Debug, thiserror::Error)]
pub enum ConversationError {
    #[error("failed to spawn claude: {0}")]
    Spawn(#[from] SpawnError),

    #[error("CLI error (exit {exit_code:?}): {message}")]
    Cli {
        message: String,
        exit_code: Option<i32>,
    },

    #[error("no result message received from CLI")]
    NoResult,

    #[error("no assistant message received — cannot produce a token")]
    NoAssistantMessage,
}

// ─── Implementation ──────────────────────────────────────────────────────────

impl ContextToken {
    /// Start a brand-new conversation.
    ///
    /// The prompt is the first user message.  `args` controls model, tools,
    /// settings, etc.  Session-control fields (`resume`, `fork_session`,
    /// `resume_session_at`) are ignored — use [`message`](Self::message)
    /// to continue an existing conversation.
    pub async fn start(
        prompt: impl Into<String>,
        args: ClaudeArgs,
    ) -> Result<Turn, ConversationError> {
        let args = ClaudeArgs {
            prompt: prompt.into(),
            // Ensure the session is persisted so we can resume from it.
            no_session_persistence: false,
            // Clear any session-control fields — this is a fresh start.
            resume: None,
            resume_session_at: None,
            fork_session: false,
            continue_session: false,
            ..args
        };

        collect_turn(args).await
    }

    /// Send a message continuing from this node, creating a child branch.
    ///
    /// Calling this multiple times on the **same** token creates sibling
    /// branches (each is an independent fork).
    pub async fn message(
        &self,
        prompt: impl Into<String>,
        args: ClaudeArgs,
    ) -> Result<Turn, ConversationError> {
        let args = ClaudeArgs {
            prompt: prompt.into(),
            no_session_persistence: false,
            resume: Some(self.session_id.clone()),
            resume_session_at: Some(self.message_id.clone()),
            fork_session: true,
            continue_session: false,
            ..args
        };

        collect_turn(args).await
    }
}

/// Run a single turn to completion and extract the token + metadata.
async fn collect_turn(args: ClaudeArgs) -> Result<Turn, ConversationError> {
    let session = spawn_claude(args)?;
    let mut stream = session.stream;

    let mut session_id: Option<String> = None;
    let mut last_assistant_uuid: Option<String> = None;
    let mut model = String::new();
    let mut result_msg: Option<ResultMessage> = None;
    let mut cli_error: Option<(String, Option<i32>)> = None;

    // Collect full assistant text from content blocks.
    let mut text_parts: Vec<String> = Vec::new();

    while let Some(item) = stream.next().await {
        match item {
            StreamItem::Message(msg) => match msg {
                Message::System(sys) => {
                    session_id = Some(sys.session_id.clone());
                    model = sys.model.clone().unwrap_or_default();
                }
                Message::Assistant(asst) => {
                    last_assistant_uuid = Some(asst.uuid.clone());
                    if model.is_empty() {
                        model = asst.message.model.clone();
                    }
                    // Extract text from content blocks.
                    for block in &asst.message.content {
                        if let ContentBlock::Text { text } = block {
                            text_parts.push(text.clone());
                        }
                    }
                }
                Message::Result(res) => {
                    result_msg = Some(res);
                }
                // Ignore streaming deltas, user messages (tool results), etc.
                _ => {}
            },
            StreamItem::Error(e) => match e {
                crate::claude::StreamError::CliError {
                    message,
                    exit_code,
                } => {
                    cli_error = Some((message, exit_code));
                }
                // Parse errors for unknown message types — ignore.
                _ => {}
            },
        }
    }

    // If we got a result, use it even if there was a stderr error
    // (the CLI sometimes writes warnings to stderr on success).
    let result = match result_msg {
        Some(res) => res,
        None => {
            if let Some((message, exit_code)) = cli_error {
                return Err(ConversationError::Cli {
                    message,
                    exit_code,
                });
            }
            return Err(ConversationError::NoResult);
        }
    };

    let sid = session_id.unwrap_or_else(|| result.session_id.clone());

    // The last assistant message UUID is our new token.
    // If there were no assistant messages (e.g. auth error), use the result UUID.
    let message_id = last_assistant_uuid.unwrap_or_else(|| result.uuid.clone());

    let usage = result.usage.as_ref();

    Ok(Turn {
        text: result.result.clone(),
        token: ContextToken {
            session_id: sid,
            message_id,
            commit_sha: None,
        },
        model,
        cost_usd: result.total_cost_usd,
        duration_ms: result.duration_ms,
        input_tokens: usage.map_or(0, |u| u.input_tokens),
        output_tokens: usage.map_or(0, |u| u.output_tokens),
        num_turns: result.num_turns,
        is_error: result.is_error,
        result_message: Some(result),
    })
}
