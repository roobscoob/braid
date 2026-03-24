use serde::{Deserialize, Serialize};

// ─── Top-Level Message Envelope ──────────────────────────────────────────────

/// Every line of NDJSON output from `claude --output-format stream-json` is one
/// of these variants. The `type` field in the JSON determines which variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    System(SystemMessage),
    Assistant(AssistantMessage),
    User(UserMessage),
    Result(ResultMessage),
    StreamEvent(StreamEventMessage),
    RateLimitEvent(RateLimitEventMessage),
}

// ─── System Message ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub subtype: String, // "init", "task_started", "task_progress", "task_completed", etc.
    pub uuid: String,
    pub session_id: String,

    // ── Fields present on "init" messages ────────────────────────────────
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
    #[serde(rename = "permissionMode", default)]
    pub permission_mode: Option<String>,
    #[serde(rename = "apiKeySource", default)]
    pub api_key_source: Option<String>,
    #[serde(default)]
    pub slash_commands: Vec<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
    #[serde(default)]
    pub output_style: Option<String>,
    #[serde(default)]
    pub skills: Vec<serde_json::Value>,
    #[serde(default)]
    pub plugins: Vec<Plugin>,

    // ── Fields present on "task_*" messages ───────────────────────────────
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub task_type: Option<String>,
    #[serde(default)]
    pub last_tool_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub path: String,
}

// ─── Assistant Message ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub uuid: String,
    pub session_id: String,
    pub parent_tool_use_id: Option<String>,
    pub message: AssistantMessageBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessageBody {
    pub model: String,
    pub id: String,
    pub role: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

// ─── Content Blocks ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        #[serde(default)]
        is_error: Option<bool>,
    },
}

// ─── User Message (Tool Results) ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub uuid: String,
    pub session_id: String,
    pub parent_tool_use_id: Option<String>,
    pub message: UserMessageBody,
    #[serde(default, deserialize_with = "deserialize_tool_use_result")]
    pub tool_use_result: Option<ToolUseResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessageBody {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

/// The `tool_use_result` field can be either a structured object or a plain
/// error string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolUseResult {
    Meta(ToolUseResultMeta),
    Error(String),
}

fn deserialize_tool_use_result<'de, D>(deserializer: D) -> Result<Option<ToolUseResult>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Deserialize;
    Option::<ToolUseResult>::deserialize(deserializer).or(Ok(None))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseResultMeta {
    #[serde(default)]
    pub filenames: Option<Vec<String>>,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u64>,
    #[serde(rename = "numFiles")]
    pub num_files: Option<u64>,
    #[serde(default)]
    pub truncated: Option<bool>,
}

// ─── Result Message ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMessage {
    pub subtype: ResultSubtype,
    pub uuid: String,
    pub session_id: String,
    pub is_error: bool,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub num_turns: u32,
    pub result: String,
    pub total_cost_usd: f64,
    #[serde(default)]
    pub usage: Option<AggregateUsage>,
    #[serde(rename = "modelUsage", default)]
    pub model_usage: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub permission_denials: Vec<PermissionDenial>,
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    Success,
    ErrorMaxTurns,
    ErrorDuringExecution,
    ErrorMaxBudgetUsd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDenial {
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: serde_json::Value,
}

// ─── Usage / Cost ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation: Option<CacheCreation>,
    #[serde(default)]
    pub service_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCreation {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub server_tool_use: Option<ServerToolUse>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerToolUse {
    #[serde(default)]
    pub web_search_requests: u64,
    #[serde(default)]
    pub web_fetch_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageEntry {
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadInputTokens", default)]
    pub cache_read_input_tokens: u64,
    #[serde(rename = "cacheCreationInputTokens", default)]
    pub cache_creation_input_tokens: u64,
    #[serde(rename = "webSearchRequests", default)]
    pub web_search_requests: u64,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "maxOutputTokens")]
    pub max_output_tokens: u64,
}

// ─── Stream Events ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEventMessage {
    pub uuid: String,
    pub session_id: String,
    pub parent_tool_use_id: Option<String>,
    pub event: StreamEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: serde_json::Value,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: Delta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: serde_json::Value,
        #[serde(default)]
        usage: Option<serde_json::Value>,
    },
    MessageStop {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta { text: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
    InputJsonDelta { partial_json: String },
}

// ─── Rate Limit Event ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitEventMessage {
    pub uuid: String,
    pub session_id: String,
    pub rate_limit_info: RateLimitInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    pub status: String,
    #[serde(rename = "resetsAt")]
    pub resets_at: u64,
    pub rate_limit_type: String,
    pub overage_status: String,
    #[serde(default)]
    pub overage_disabled_reason: Option<String>,
    #[serde(default)]
    pub is_using_overage: bool,
}

// ─── MCP Configuration ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: std::collections::HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
    Remote {
        #[serde(rename = "type")]
        transport_type: String, // "sse" | "http"
        url: String,
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
    },
}
