//! Typed representation of Claude Code's `settings.json` schema.
//!
//! Every field is `Option` or has a `#[serde(default)]` so that partial configs
//! round-trip correctly.  Unrecognised keys are captured by `extra` on the root
//! struct (the schema sets `additionalProperties: true`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ─── Root ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    // ── Auth / Keys ──────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_helper: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_credential_export: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_auth_refresh: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_login_method: Option<ForceLoginMethod>,
    #[serde(rename = "forceLoginOrgUUID", skip_serializing_if = "Option::is_none")]
    pub force_login_org_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_headers_helper: Option<String>,

    // ── Model / Effort ───────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_level: Option<EffortLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode_per_session_opt_in: Option<bool>,

    // ── Prompts ──────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,

    // ── Memory / Cleanup ─────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_memory_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_period_days: Option<u32>,

    // ── Git / Attribution ────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_git_instructions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_co_authored_by: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<Attribution>,

    // ── Plans ────────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plans_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub respect_gitignore: Option<bool>,

    // ── Environment ──────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    // ── Permissions ──────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,

    // ── MCP ──────────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_all_project_mcp_servers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_mcpjson_servers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_mcpjson_servers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mcp_servers: Option<Vec<McpServerMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied_mcp_servers: Option<Vec<McpServerMatcher>>,

    // ── Hooks ────────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<Hooks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_all_hooks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_managed_hooks_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_managed_permission_rules_only: Option<bool>,

    // ── Status Line ──────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_line: Option<StatusLine>,

    // ── File Suggestion ──────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_suggestion: Option<FileSuggestion>,

    // ── Plugins / Marketplaces ───────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_plugins: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_known_marketplaces: Option<HashMap<String, MarketplaceEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_known_marketplaces: Option<Vec<MarketplaceSource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_marketplaces: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_plugins: Option<Vec<String>>,

    // ── Sandbox ──────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<Sandbox>,

    // ── UI / Spinner ─────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spinner_verbs: Option<SpinnerVerbs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spinner_tips_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spinner_tips_override: Option<SpinnerTipsOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_theme: Option<TerminalTheme>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rich_text: Option<bool>,

    // ── WebFetch ─────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_web_fetch_preflight: Option<bool>,

    // ── Catch-all for unknown keys ───────────────────────────────────────
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ─── Enums ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForceLoginMethod {
    #[serde(rename = "claudeai")]
    ClaudeAi,
    Console,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalTheme {
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DefaultPermissionMode {
    Default,
    AcceptEdits,
    BypassPermissions,
    Plan,
    Delegate,
    DontAsk,
}

// ─── Attribution ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Attribution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

// ─── Permissions ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<DefaultPermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_bypass_permissions_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<Vec<String>>,
}

// ─── MCP Server Matcher ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerMatcher {
    ByName {
        #[serde(rename = "serverName")]
        server_name: String,
    },
    ByCommand {
        #[serde(rename = "serverCommand")]
        server_command: Vec<String>,
    },
    ByUrl {
        #[serde(rename = "serverUrl")]
        server_url: String,
    },
}

// ─── Hooks ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Hooks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_tool_use: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_tool_use: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_tool_use_failure: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_request: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_prompt_submit: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_start: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_stop: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_compact: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teammate_idle: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_completed: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions_loaded: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_change: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_create: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_remove: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start: Option<Vec<HookMatcher>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_end: Option<Vec<HookMatcher>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookMatcher {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookCommand {
    Command {
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<f64>,
        #[serde(rename = "async", skip_serializing_if = "Option::is_none")]
        is_async: Option<bool>,
        #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
        status_message: Option<String>,
    },
    Prompt {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<f64>,
        #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
        status_message: Option<String>,
    },
    Agent {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<f64>,
        #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
        status_message: Option<String>,
    },
    Http {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
        #[serde(rename = "allowedEnvVars", skip_serializing_if = "Option::is_none")]
        allowed_env_vars: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<f64>,
        #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
        status_message: Option<String>,
    },
}

// ─── Status Line / File Suggestion ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusLine {
    #[serde(rename = "type")]
    pub kind: String, // "command"
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSuggestion {
    #[serde(rename = "type")]
    pub kind: String, // "command"
    pub command: String,
}

// ─── Sandbox ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sandbox {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<SandboxNetwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<SandboxFilesystem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_violations: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded_commands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_allow_bash_if_sandboxed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_weaker_network_isolation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_weaker_nested_sandbox: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_unsandboxed_commands: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxNetwork {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_unix_sockets: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_all_unix_sockets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_local_binding: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_proxy_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socks_proxy_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_managed_domains_only: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxFilesystem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_write: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_write: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_read: Option<Vec<String>>,
}

// ─── Spinner ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpinnerVerbs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SpinnerVerbsMode>,
    pub verbs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpinnerVerbsMode {
    Append,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpinnerTipsOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SpinnerTipsMode>,
    pub tips: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpinnerTipsMode {
    Append,
    Replace,
}

// ─── Marketplace Sources ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    pub source: MarketplaceSource,
    #[serde(rename = "installLocation", skip_serializing_if = "Option::is_none")]
    pub install_location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum MarketplaceSource {
    Url {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
    },
    HostPattern {
        #[serde(rename = "hostPattern")]
        host_pattern: String,
    },
    Github {
        repo: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Git {
        url: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Npm {
        package: String,
    },
    File {
        path: String,
    },
    Directory {
        path: String,
    },
    PathPattern {
        #[serde(rename = "pathPattern")]
        path_pattern: String,
    },
}
