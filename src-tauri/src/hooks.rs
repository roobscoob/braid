//! Typed representations of Claude Code hook stdin/stdout payloads.
//!
//! When Claude Code invokes a hook it pipes JSON to stdin and reads JSON from
//! stdout.  These types model every documented event.

use serde::{Deserialize, Serialize};

// ─── Hook Input (stdin) ──────────────────────────────────────────────────────

/// Fields common to every hook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInputCommon {
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_type: Option<String>,
}

/// Typed union of every hook event's stdin payload.
///
/// Deserialized via the `hook_event_name` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum HookInput {
    // ── Session ──────────────────────────────────────────────────────────
    SessionStart {
        session_id: String,
        transcript_path: String,
        cwd: String,
        source: String, // "startup" | "resume" | "clear" | "compact"
        model: String,
    },

    SessionEnd {
        session_id: String,
        transcript_path: String,
        cwd: String,
        reason: String, // "clear" | "resume" | "logout" | "prompt_input_exit" | ...
    },

    // ── Prompt ───────────────────────────────────────────────────────────
    UserPromptSubmit {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        prompt: String,
    },

    // ── Tool lifecycle ───────────────────────────────────────────────────
    PreToolUse {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        tool_name: String,
        tool_use_id: String,
        tool_input: serde_json::Value,
    },

    PostToolUse {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        tool_name: String,
        tool_use_id: String,
        tool_input: serde_json::Value,
        tool_response: serde_json::Value,
    },

    PostToolUseFailure {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        tool_name: String,
        tool_use_id: String,
        tool_input: serde_json::Value,
        error: String,
        #[serde(default)]
        is_interrupt: Option<bool>,
    },

    PermissionRequest {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        tool_name: String,
        tool_input: serde_json::Value,
        #[serde(default)]
        permission_suggestions: Vec<PermissionSuggestion>,
    },

    // ── Notification ─────────────────────────────────────────────────────
    Notification {
        session_id: String,
        transcript_path: String,
        cwd: String,
        message: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        notification_type: Option<String>,
    },

    // ── Agents ───────────────────────────────────────────────────────────
    SubagentStart {
        session_id: String,
        transcript_path: String,
        cwd: String,
        agent_id: String,
        agent_type: String,
    },

    SubagentStop {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        #[serde(default)]
        stop_hook_active: bool,
        agent_id: String,
        agent_type: String,
        agent_transcript_path: String,
        last_assistant_message: String,
    },

    // ── Stop ─────────────────────────────────────────────────────────────
    Stop {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        #[serde(default)]
        stop_hook_active: bool,
        last_assistant_message: String,
    },

    StopFailure {
        session_id: String,
        transcript_path: String,
        cwd: String,
        error: String, // "rate_limit" | "authentication_failed" | ...
        #[serde(default)]
        error_details: Option<String>,
        last_assistant_message: String,
    },

    // ── Teams / Tasks ────────────────────────────────────────────────────
    TeammateIdle {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        teammate_name: String,
        team_name: String,
    },

    TaskCompleted {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        task_id: String,
        task_subject: String,
        #[serde(default)]
        task_description: Option<String>,
        #[serde(default)]
        teammate_name: Option<String>,
        #[serde(default)]
        team_name: Option<String>,
    },

    // ── Instructions ─────────────────────────────────────────────────────
    InstructionsLoaded {
        session_id: String,
        transcript_path: String,
        cwd: String,
        file_path: String,
        memory_type: String, // "User" | "Project" | "Local" | "Managed"
        load_reason: String, // "session_start" | "nested_traversal" | ...
        #[serde(default)]
        globs: Option<Vec<String>>,
        #[serde(default)]
        trigger_file_path: Option<String>,
        #[serde(default)]
        parent_file_path: Option<String>,
    },

    // ── Config ───────────────────────────────────────────────────────────
    ConfigChange {
        session_id: String,
        transcript_path: String,
        cwd: String,
        source: String, // "user_settings" | "project_settings" | ...
        #[serde(default)]
        file_path: Option<String>,
    },

    // ── Worktree ─────────────────────────────────────────────────────────
    WorktreeCreate {
        session_id: String,
        transcript_path: String,
        cwd: String,
        name: String,
    },

    WorktreeRemove {
        session_id: String,
        transcript_path: String,
        cwd: String,
        worktree_path: String,
    },

    // ── Compact ──────────────────────────────────────────────────────────
    PreCompact {
        session_id: String,
        transcript_path: String,
        cwd: String,
        trigger: String, // "manual" | "auto"
        #[serde(default)]
        custom_instructions: Option<String>,
    },

    PostCompact {
        session_id: String,
        transcript_path: String,
        cwd: String,
        trigger: String,
        #[serde(default)]
        compact_summary: Option<String>,
    },

    // ── Elicitation ──────────────────────────────────────────────────────
    Elicitation {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        mcp_server_name: String,
        message: String,
        #[serde(default)]
        mode: Option<String>, // "form" | "url"
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        elicitation_id: Option<String>,
        #[serde(default)]
        requested_schema: Option<serde_json::Value>,
    },

    ElicitationResult {
        session_id: String,
        transcript_path: String,
        cwd: String,
        permission_mode: String,
        mcp_server_name: String,
        action: String, // "accept" | "decline" | "cancel"
        #[serde(default)]
        content: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSuggestion {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub rules: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub behavior: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub directories: Option<Vec<String>>,
    pub destination: String,
}

// ─── Hook Output (stdout) ────────────────────────────────────────────────────

/// Common output fields that any hook can return.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookOutput {
    /// Set to `false` to abort the session.
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub should_continue: Option<bool>,

    /// Reason shown to the user when `continue` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,

    /// Suppress the hook's stdout from appearing in the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppress_output: Option<bool>,

    /// Inject a system message into the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,

    /// Block the action (for PostToolUse, UserPromptSubmit, Stop, SubagentStop, ConfigChange).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>, // "block"

    /// Reason shown when `decision` is "block".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Event-specific output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

/// The `hookSpecificOutput` object — its shape depends on the event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "hookEventName")]
pub enum HookSpecificOutput {
    PreToolUse {
        /// "allow" | "deny" | "ask"
        #[serde(rename = "permissionDecision")]
        permission_decision: String,
        #[serde(rename = "permissionDecisionReason")]
        permission_decision_reason: String,
        /// Optionally rewrite the tool's input.
        #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },

    PostToolUse {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
        /// Rewrite an MCP tool's output.
        #[serde(rename = "updatedMCPToolOutput", skip_serializing_if = "Option::is_none")]
        updated_mcp_tool_output: Option<serde_json::Value>,
    },

    PostToolUseFailure {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },

    UserPromptSubmit {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },

    SessionStart {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },

    Notification {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },

    SubagentStart {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },

    PermissionRequest {
        decision: PermissionRequestDecision,
    },

    Elicitation {
        action: String, // "accept" | "decline" | "cancel"
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
    },

    ElicitationResult {
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequestDecision {
    pub behavior: String, // "allow" | "deny"
    #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    #[serde(rename = "updatedPermissions", skip_serializing_if = "Option::is_none")]
    pub updated_permissions: Option<Vec<PermissionSuggestion>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
}

// ─── Convenience constructors ────────────────────────────────────────────────

impl HookOutput {
    /// Allow the tool call to proceed.
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: "allow".into(),
                permission_decision_reason: reason.into(),
                updated_input: None,
                additional_context: None,
            }),
            ..Default::default()
        }
    }

    /// Deny the tool call.
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: "deny".into(),
                permission_decision_reason: reason.into(),
                updated_input: None,
                additional_context: None,
            }),
            ..Default::default()
        }
    }

    /// Ask the user to confirm.
    pub fn ask(reason: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: "ask".into(),
                permission_decision_reason: reason.into(),
                updated_input: None,
                additional_context: None,
            }),
            ..Default::default()
        }
    }

    /// Block a post-tool-use or other blockable event.
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            decision: Some("block".into()),
            reason: Some(reason.into()),
            ..Default::default()
        }
    }
}
