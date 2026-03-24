// ─── Events emitted from Rust → Frontend ────────────────────────────────────

/** Discriminated union of all events the backend emits for a turn. */
export type TurnEvent =
  | { type: "started"; turn_id: string }
  | { type: "system_init"; turn_id: string; session_id: string; model: string }
  | { type: "text_delta"; turn_id: string; text: string; parent_tool_use_id?: string | null }
  | { type: "thinking_delta"; turn_id: string; text: string; parent_tool_use_id?: string | null }
  | { type: "tool_use_start"; turn_id: string; tool_name: string; tool_id: string; parent_tool_use_id?: string | null }
  | { type: "tool_use_input_delta"; turn_id: string; tool_id: string; partial_json: string }
  | { type: "tool_result"; turn_id: string; tool_id: string; content: string; is_error: boolean; parent_tool_use_id?: string | null }
  | {
      type: "finished";
      turn_id: string;
      session_id: string;
      message_id: string;
      model: string;
      cost_usd: number;
      duration_ms: number;
      input_tokens: number;
      output_tokens: number;
      num_turns: number;
      is_error: boolean;
      result_text: string;
      commit_sha?: string | null;
    }
  | { type: "agent_progress"; turn_id: string; agent_tool_id: string; tool_name: string; description: string }
  | { type: "resume"; turn_id: string }
  | { type: "error"; turn_id: string; message: string };

// ─── Hook Decision Request ───────────────────────────────────────────────────

export interface HookDecisionRequest {
  request_id: string;
  node_id: string | null;
  event: string;
  tool_name: string | null;
  tool_input: Record<string, unknown> | null;
}

// ─── Tree Node ───────────────────────────────────────────────────────────────

export interface ContentBlock {
  type: "text" | "thinking" | "tool_use" | "tool_result" | "separator" | "progress";
  text?: string;
  tool_name?: string;
  tool_id?: string;
  input_json?: string;
  content?: string;
  is_error?: boolean;
  /** If set, this block belongs to a subagent spawned by the tool_use with this id. */
  parent_tool_use_id?: string | null;
  /** Nested blocks from a subagent (only on tool_use blocks for Agent/Task). */
  children?: ContentBlock[];
}

export interface TreeNode {
  id: string; // message_id (or temp id while streaming)
  session_id: string;
  prompt: string;
  /** Structured content blocks — text, thinking, tool calls, results. */
  blocks: ContentBlock[];
  model: string;
  cost_usd: number;
  parent_id: string | null;
  children: string[];
  is_error: boolean;
  /** True while the agent is still generating. */
  streaming: boolean;
  /** Monotonic creation index (higher = newer). */
  createdAt: number;
  /** Git commit SHA of the code state at this node. */
  commit_sha?: string | null;
}
