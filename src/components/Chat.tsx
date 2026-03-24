import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useStore, computeChain } from "../store";
import type { TurnEvent, ContentBlock, HookDecisionRequest } from "../types";

export function Chat() {
  const [input, setInput] = useState("");
  const [turnError, setTurnError] = useState<string | null>(null);
  const nodes = useStore((s) => s.nodes);
  const selectedId = useStore((s) => s.selectedId);
  const rootId = useStore((s) => s.rootId);
  const pending = useStore((s) => s.pending);
  const beginTurn = useStore((s) => s.beginTurn);
  const handleEvent = useStore((s) => s.handleEvent);
  const hookRequests = useStore((s) => s.hookRequests);
  const addHookRequest = useStore((s) => s.addHookRequest);
  const removeHookRequest = useStore((s) => s.removeHookRequest);
  const chain = useMemo(
    () => computeChain(nodes, selectedId),
    [nodes, selectedId]
  );
  const isStreaming = pending.size > 0;
  const bottomRef = useRef<HTMLDivElement>(null);

  // Listen for backend events.
  useEffect(() => {
    const unlisten = listen<TurnEvent>("turn-event", (e) => {
      console.log("[turn-event]", e.payload);
      handleEvent(e.payload);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [handleEvent]);

  // Listen for hook decision requests.
  useEffect(() => {
    const unlisten = listen<HookDecisionRequest>("hook-decision-request", (e) => {
      console.log("[hook-request]", e.payload);
      addHookRequest(e.payload);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [addHookRequest]);

  // Auto-scroll to bottom on new content.
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [chain, pending]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const prompt = input.trim();
    if (!prompt || isStreaming) return;
    setInput("");
    setTurnError(null);

    try {
      // TODO: replace with a project picker
      const projectPath = ".";

      if (!rootId) {
        const turnId = await invoke<string>("start_conversation", {
          prompt,
          projectPath,
        });
        beginTurn(turnId, null, prompt);
      } else {
        const parent = selectedId
          ? useStore.getState().nodes.get(selectedId)
          : null;
        if (!parent) return;

        const turnId = await invoke<string>("send_message", {
          sessionId: parent.session_id,
          messageId: parent.id,
          prompt,
          projectPath,
          commitSha: parent.commit_sha ?? null,
        });
        beginTurn(turnId, parent.id, prompt);
      }
    } catch (err) {
      setTurnError(String(err));
    }
  };

  return (
    <div className="flex flex-col h-full bg-zinc-950">
      {/* Messages */}
      <div className="flex-1 overflow-y-auto p-6 space-y-6">
        {chain.length === 0 && !isStreaming && (
          <div className="flex items-center justify-center h-full text-zinc-500">
            <p className="text-lg">Start a conversation</p>
          </div>
        )}
        {chain.map((node) => (
          <div key={node.id} className="space-y-3">
            {/* User message */}
            <div className="flex justify-end">
              <div className="max-w-[70%] rounded-2xl rounded-tr-sm bg-indigo-600 px-4 py-2.5 text-sm text-white whitespace-pre-wrap">
                {node.prompt}
              </div>
            </div>
            {/* Assistant blocks */}
            {node.blocks.length > 0 && (
              <div className="flex justify-start">
                <div className="max-w-[80%] space-y-0.5 flex flex-col items-start">
                  {(() => {
                    const groups = groupBlocks(node.blocks);
                    return groups.map((group, i) => (
                      <GroupedBlockView
                        key={i}
                        group={group}
                        isError={node.is_error}
                        position={
                          groups.length === 1
                            ? "only"
                            : i === 0
                              ? "first"
                              : i === groups.length - 1
                                ? "last"
                                : "middle"
                        }
                      />
                    ));
                  })()}
                  {/* Streaming cursor */}
                  {node.streaming && (
                    <span className="inline-flex gap-1 text-zinc-500 text-sm px-1">
                      <span className="animate-pulse">▍</span>
                    </span>
                  )}
                  {/* Meta line */}
                  {!node.streaming && node.model && (
                    <div className="text-[10px] text-zinc-600 px-4">
                      {node.model}
                      {node.cost_usd > 0 && ` · $${node.cost_usd.toFixed(4)}`}
                      {node.children.length > 0 &&
                        ` · ${node.children.length} branch${node.children.length > 1 ? "es" : ""}`}
                    </div>
                  )}
                </div>
              </div>
            )}
            {/* Show typing indicator if streaming but no blocks yet */}
            {node.streaming && node.blocks.length === 0 && (
              <div className="flex justify-start">
                <div className="rounded-2xl rounded-tl-sm bg-zinc-800 px-4 py-3 text-sm text-zinc-400">
                  <span className="inline-flex gap-1">
                    <span className="animate-bounce" style={{ animationDelay: "0ms" }}>·</span>
                    <span className="animate-bounce" style={{ animationDelay: "150ms" }}>·</span>
                    <span className="animate-bounce" style={{ animationDelay: "300ms" }}>·</span>
                  </span>
                </div>
              </div>
            )}
          </div>
        ))}
        {/* Hook approval requests */}
        {hookRequests.map((req) => (
          <HookApproval
            key={req.request_id}
            request={req}
            onResolve={(requestId: string, allow: boolean) => {
              const response = allow
                ? JSON.stringify({
                    hookSpecificOutput: {
                      hookEventName: "PreToolUse",
                      permissionDecision: "allow",
                      permissionDecisionReason: "approved by user",
                    },
                  })
                : JSON.stringify({
                    hookSpecificOutput: {
                      hookEventName: "PreToolUse",
                      permissionDecision: "deny",
                      permissionDecisionReason: "denied by user",
                    },
                  });
              invoke("resolve_hook", { requestId, responseJson: response });
              removeHookRequest(requestId);
            }}
          />
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Turn start error */}
      {turnError && (
        <div className="mx-4 mb-2 rounded-xl bg-red-950/40 border border-red-800/60 px-4 py-2.5 text-sm text-red-300 flex items-start gap-2">
          <span className="shrink-0 mt-0.5">✕</span>
          <span className="flex-1">{turnError}</span>
          <button
            onClick={() => setTurnError(null)}
            className="shrink-0 text-red-400 hover:text-red-200 text-xs"
          >
            dismiss
          </button>
        </div>
      )}

      {/* Input */}
      <form onSubmit={handleSubmit} className="border-t border-zinc-800 p-4">
        <div className="flex gap-3">
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder={
              rootId ? "Send a message\u2026" : "Start a new conversation\u2026"
            }
            disabled={isStreaming}
            className="flex-1 rounded-xl bg-zinc-800 border border-zinc-700 px-4 py-2.5 text-sm text-zinc-100 placeholder-zinc-500 outline-none focus:border-indigo-500 focus:ring-1 focus:ring-indigo-500 disabled:opacity-50"
          />
          <button
            type="submit"
            disabled={isStreaming || !input.trim()}
            className="rounded-xl bg-indigo-600 px-5 py-2.5 text-sm font-medium text-white hover:bg-indigo-500 disabled:opacity-50 disabled:hover:bg-indigo-600 transition-colors"
          >
            Send
          </button>
        </div>
      </form>
    </div>
  );
}

// ─── Group tool_use + tool_result by tool_id ────────────────────────────────

interface ToolCallEntry {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
}

interface GroupedBlock {
  kind: "text" | "thinking" | "tool_call" | "tool_group" | "progress";
  block?: ContentBlock;
  // For single tool_call:
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  // For tool_group:
  toolName?: string;
  entries?: ToolCallEntry[];
}

/** Merge sequential tool_use/tool_result pairs, then collapse consecutive same-name calls into groups.
 *  Set `flat` to skip the grouping step (used for agent children). */
function groupBlocks(blocks: ContentBlock[], flat = false): GroupedBlock[] {
  // Step 1: pair tool_use with tool_result by tool_id.
  const paired: GroupedBlock[] = [];
  const resultByToolId = new Map<string, ContentBlock>();

  for (const b of blocks) {
    if (b.type === "tool_result" && b.tool_id) {
      resultByToolId.set(b.tool_id, b);
    }
  }

  for (const b of blocks) {
    if (b.type === "tool_use") {
      paired.push({
        kind: "tool_call",
        toolUse: b,
        toolResult: b.tool_id ? resultByToolId.get(b.tool_id) : undefined,
      });
    } else if (b.type === "tool_result") {
      if (!b.tool_id || !blocks.some((x) => x.type === "tool_use" && x.tool_id === b.tool_id)) {
        paired.push({ kind: "tool_call", toolResult: b });
      }
    } else if (b.type === "separator") {
      // Invisible — prevents text merging in the store.
    } else if (b.type === "progress") {
      paired.push({ kind: "progress", block: b });
    } else {
      paired.push({ kind: b.type as "text" | "thinking", block: b });
    }
  }

  // Step 2: collapse consecutive same-name tool_call entries into tool_groups.
  if (flat) return paired;
  const groups: GroupedBlock[] = [];
  let i = 0;
  while (i < paired.length) {
    const current = paired[i];
    if (current.kind === "tool_call" && current.toolUse?.tool_name && current.toolUse.tool_name !== "Agent") {
      const name = current.toolUse.tool_name;
      const run: ToolCallEntry[] = [
        { toolUse: current.toolUse, toolResult: current.toolResult },
      ];
      // Gather consecutive calls with the same tool name.
      while (i + 1 < paired.length) {
        const next = paired[i + 1];
        if (next.kind === "tool_call" && next.toolUse?.tool_name === name) {
          run.push({ toolUse: next.toolUse, toolResult: next.toolResult });
          i++;
        } else {
          break;
        }
      }
      if (run.length === 1) {
        // Single call — keep as tool_call.
        groups.push(current);
      } else {
        // Multiple consecutive — collapse into a group.
        groups.push({
          kind: "tool_group",
          toolName: name,
          entries: run,
        });
      }
    } else {
      groups.push(current);
    }
    i++;
  }

  return groups;
}

// ─── Block renderers ─────────────────────────────────────────────────────────

type BlockPosition = "only" | "first" | "middle" | "last";

/** Left-side border radius based on position in the bubble stack. Right side stays fully rounded. */
function bubbleRadius(pos: BlockPosition) {
  switch (pos) {
    case "only": return "rounded-2xl";                                  // all rounded
    case "first": return "rounded-r-2xl rounded-tl-2xl rounded-bl-sm";   // top rounded, bottom-left tight
    case "middle": return "rounded-r-2xl rounded-l-sm";                   // both left corners tight
    case "last": return "rounded-r-2xl rounded-tl-sm rounded-bl-2xl";   // top-left tight, bottom rounded
  }
}

/** Same idea but for tool call containers (which use rounded-xl). */
function toolRadius(pos: BlockPosition) {
  switch (pos) {
    case "only": return "rounded-xl";
    case "first": return "rounded-r-xl rounded-tl-xl rounded-bl-sm";
    case "middle": return "rounded-r-xl rounded-l-sm";
    case "last": return "rounded-r-xl rounded-tl-sm rounded-bl-xl";
  }
}

function GroupedBlockView({ group, isError, position = "only" }: { group: GroupedBlock; isError?: boolean; position?: BlockPosition }) {
  switch (group.kind) {
    case "text":
      return (
        <div className={`${bubbleRadius(position)} px-4 py-2.5 text-sm whitespace-pre-wrap ${isError
          ? "bg-red-950/40 border border-red-800/60 text-red-300"
          : "bg-zinc-800 text-zinc-200"
          }`}>
          {group.block?.text}
        </div>
      );

    case "thinking":
      return (
        <div className={`${bubbleRadius(position)} border border-zinc-700 bg-transparent px-4 py-2.5 text-sm text-zinc-400 whitespace-pre-wrap italic`}>
          {group.block?.text}
        </div>
      );

    case "tool_call":
      return <ToolCallView toolUse={group.toolUse} toolResult={group.toolResult} position={position} />;

    case "tool_group":
      return <ToolGroupView toolName={group.toolName!} entries={group.entries!} position={position} />;

    case "progress":
      return <ProgressDot block={group.block!} />;

    default:
      return null;
  }
}

function ToolCallView({
  toolUse,
  toolResult,
  position = "only",
}: {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  position?: BlockPosition;
}) {
  const toolName = toolUse?.tool_name ?? "tool";

  // Dispatch to specialized renderers.
  switch (toolName) {
    case "Read":
      return <ReadToolView toolUse={toolUse} toolResult={toolResult} position={position} />;
    case "Bash":
      return <BashToolView toolUse={toolUse} toolResult={toolResult} position={position} />;
    case "Glob":
      return <GlobToolView toolUse={toolUse} toolResult={toolResult} position={position} />;
    case "Agent":
      return <AgentToolView toolUse={toolUse} toolResult={toolResult} position={position} />;
    default:
      return <GenericToolView toolUse={toolUse} toolResult={toolResult} position={position} />;
  }
}

// ─── Shared helpers ──────────────────────────────────────────────────────────

function parseInput(toolUse?: ContentBlock): Record<string, unknown> {
  if (!toolUse?.input_json) return {};
  try {
    return JSON.parse(toolUse.input_json);
  } catch {
    // Streaming — JSON is incomplete. Try to extract known keys with regex.
    const raw = toolUse.input_json;
    const result: Record<string, unknown> = {};
    // Match "key": "value" patterns (handles escaped quotes).
    for (const m of raw.matchAll(/"(\w+)"\s*:\s*"((?:[^"\\]|\\.)*)"/g)) {
      result[m[1]] = m[2].replace(/\\"/g, '"').replace(/\\\\/g, "\\");
    }
    // Match "key": number patterns.
    for (const m of raw.matchAll(/"(\w+)"\s*:\s*(\d+(?:\.\d+)?)/g)) {
      result[m[1]] = Number(m[2]);
    }
    return result;
  }
}

function Chevron({ expanded }: { expanded: boolean }) {
  return (
    <svg
      className={`w-3 h-3 text-zinc-500 transition-transform ${expanded ? "rotate-90" : ""}`}
      viewBox="0 0 16 16"
    >
      <path d="M6 4l4 4-4 4" stroke="currentColor" strokeWidth="1.5" fill="none" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** Map tool names to active/done verb labels. */
/** [active, done, failed] verb labels per tool. */
const TOOL_VERBS: Record<string, [string, string, string]> = {
  Read: ["Reading…", "Read", "Failed to read"],
  Edit: ["Editing…", "Edited", "Failed to edit"],
  Write: ["Writing…", "Wrote", "Failed to write"],
  Bash: ["Running…", "Ran", "Failed to run"],
  Glob: ["Searching…", "Found", "Failed to find"],
  Grep: ["Searching…", "Searched", "Failed to search"],
  WebFetch: ["Fetching…", "Fetched", "Failed to fetch"],
  WebSearch: ["Searching…", "Searched", "Failed to search"],
  Agent: ["Running agent…", "Agent", "Agent failed"],
};

function toolVerb(toolName: string, done: boolean, isError?: boolean): string {
  const verbs = TOOL_VERBS[toolName];
  if (verbs) {
    if (!done) return verbs[0];
    return isError ? verbs[2] : verbs[1];
  }
  if (!done) return `${toolName}…`;
  return isError ? `Failed: ${toolName}` : toolName;
}

function toolBorderClass(toolResult?: ContentBlock) {
  if (!toolResult) return "border-amber-800/40 bg-zinc-900";
  if (toolResult.is_error) return "border-red-800/60 bg-red-950/40";
  return "border-zinc-800 bg-zinc-900";
}

/** Returns contextual text classes that look good on both normal and error backgrounds. */
function toolTextClasses(toolResult?: ContentBlock) {
  const isErr = toolResult?.is_error ?? false;
  return {
    label: isErr ? "text-red-400" : "text-zinc-500",      // "Read", "$"
    primary: isErr ? "text-red-300" : "text-zinc-300",     // filename, command
    secondary: isErr ? "text-red-400/70" : "text-zinc-600", // line count, path
    content: isErr ? "text-red-400" : "text-zinc-400",     // expanded body
    border: isErr ? "border-red-800/40" : "border-zinc-800",
    hover: isErr ? "hover:bg-red-900/30" : "hover:bg-zinc-800/50",
  };
}

// ─── Read tool ───────────────────────────────────────────────────────────────

function ReadToolView({
  toolUse,
  toolResult,
  position = "only",
}: {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  position?: BlockPosition;
}) {
  const [expanded, setExpanded] = useState(false);
  const input = parseInput(toolUse);
  const filePath = (input.file_path as string) ?? "unknown file";
  const fileName = filePath.split(/[/\\]/).pop() ?? filePath;
  const lineCount = toolResult?.content?.split("\n").length ?? 0;
  const tc = toolTextClasses(toolResult);

  const isDone = !!toolResult;
  const isErr = toolResult?.is_error ?? false;

  return (
    <div className={`${toolRadius(position)} border text-xs overflow-hidden ${toolBorderClass(toolResult)} ${!isDone ? "tool-shimmer" : ""}`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className={`w-full flex items-center gap-2 px-4 py-2 text-left ${tc.hover} transition-colors`}
      >
        <Chevron expanded={expanded} />
        <span className={isErr ? tc.label : tc.label}>
          {toolVerb("Read", isDone, isErr)}
        </span>
        <span className={`font-mono truncate ${tc.primary}`} title={filePath}>
          {fileName}
        </span>
        {isDone && !isErr && lineCount > 0 && (
          <span className={`text-[10px] ${tc.secondary}`}>{lineCount} lines</span>
        )}
      </button>

      {expanded && (
        <div className={`border-t ${tc.border}`}>
          <div className={`px-4 py-1.5 text-[10px] font-mono truncate ${tc.secondary}`}>
            {filePath}
          </div>
          {toolResult && (
            <pre className={`px-4 py-2 overflow-x-auto max-h-64 overflow-y-auto font-mono whitespace-pre text-[11px] leading-relaxed ${tc.content}`}>
              {toolResult.content}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Glob tool ───────────────────────────────────────────────────────────────

function GlobToolView({
  toolUse,
  toolResult,
  position = "only",
}: {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  position?: BlockPosition;
}) {
  const [expanded, setExpanded] = useState(false);
  const input = parseInput(toolUse);
  const pattern = (input.pattern as string) ?? "*";
  const searchPath = input.path as string | undefined;
  const tc = toolTextClasses(toolResult);
  const isDone = !!toolResult;
  const isErr = toolResult?.is_error ?? false;

  // Parse result into file list.
  const files = isDone && !isErr && toolResult?.content
    ? toolResult.content.split("\n").filter(Boolean)
    : [];
  const fileCount = files.length;

  return (
    <div className={`${toolRadius(position)} border text-xs overflow-hidden ${toolBorderClass(toolResult)} ${!isDone ? "tool-shimmer" : ""}`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className={`w-full flex items-center gap-2 px-4 py-2 text-left ${tc.hover} transition-colors`}
      >
        <Chevron expanded={expanded} />
        <span className={tc.label}>{toolVerb("Glob", isDone, isErr)}</span>
        <code className={`font-mono truncate ${tc.primary}`} title={pattern}>
          {pattern}
        </code>
        {isDone && !isErr && (
          <span className={`text-[10px] ${tc.secondary}`}>
            {fileCount} file{fileCount !== 1 ? "s" : ""}
          </span>
        )}
      </button>

      {expanded && (
        <div className={`border-t ${tc.border}`}>
          {searchPath && (
            <div className={`px-4 py-1.5 text-[10px] font-mono truncate ${tc.secondary}`}>
              in {searchPath}
            </div>
          )}
          {isDone && (
            <div className="px-4 py-2 max-h-48 overflow-y-auto">
              {isErr ? (
                <pre className={`whitespace-pre-wrap text-[11px] ${tc.content}`}>
                  {toolResult?.content}
                </pre>
              ) : (
                <ul className="space-y-0.5">
                  {files.map((f, i) => {
                    const name = f.split(/[/\\]/).pop() ?? f;
                    return (
                      <li key={i} className={`font-mono text-[11px] truncate ${tc.content}`} title={f}>
                        {name}
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Bash tool ───────────────────────────────────────────────────────────────

function BashToolView({
  toolUse,
  toolResult,
  position = "only",
}: {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  position?: BlockPosition;
}) {
  const [expanded, setExpanded] = useState(false);
  const input = parseInput(toolUse);
  const command = (input.command as string) ?? "";
  const description = input.description as string | undefined;
  const outputLines = toolResult?.content?.split("\n").length ?? 0;
  const shortCmd = command.length <= 60 ? command : command.slice(0, 57) + "…";
  const tc = toolTextClasses(toolResult);

  const isDone = !!toolResult;
  const isErr = toolResult?.is_error ?? false;

  return (
    <div className={`${toolRadius(position)} border text-xs overflow-hidden ${toolBorderClass(toolResult)} ${!isDone ? "tool-shimmer" : ""}`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className={`w-full flex items-center gap-2 px-4 py-2 text-left ${tc.hover} transition-colors`}
      >
        <Chevron expanded={expanded} />
        <span className={tc.label}>{toolVerb("Bash", isDone, isErr)}</span>
        <code className={`font-mono truncate ${tc.primary}`} title={command}>
          {description ?? shortCmd}
        </code>
        {isDone && !isErr && outputLines > 1 && (
          <span className={`text-[10px] ${tc.secondary}`}>{outputLines} lines</span>
        )}
      </button>

      {expanded && (
        <div className={`border-t ${tc.border}`}>
          {command.length > 60 && (
            <pre className={`px-4 py-1.5 text-[10px] font-mono whitespace-pre-wrap ${tc.secondary}`}>
              $ {command}
            </pre>
          )}
          {toolResult && (
            <pre className={`px-4 py-2 overflow-x-auto max-h-64 overflow-y-auto font-mono whitespace-pre text-[11px] leading-relaxed ${tc.content}`}>
              {toolResult.content}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Progress dot (background agent status line) ─────────────────────────────

function ProgressDot({ block }: { block: ContentBlock }) {
  return (
    <div className="flex items-center gap-2 px-1 py-0.5 text-xs text-zinc-500">
      <span className="w-1.5 h-1.5 rounded-full shrink-0 bg-zinc-600" />
      <span>{block.text}</span>
    </div>
  );
}

// ─── Shared indented children layout ─────────────────────────────────────────

/** Collapsed tail view — shows last ~50px of children, pinned to the bottom,
 *  with a fade at top. Smoothly animates when new items push in. */
function CollapsedTail({ collapseGen, children }: { collapseGen: number; children: React.ReactNode }) {
  const innerRef = useRef<HTMLDivElement>(null);
  const prevHeight = useRef(0);

  useEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const newHeight = el.scrollHeight;
    if (prevHeight.current > 0 && newHeight !== prevHeight.current) {
      const diff = newHeight - prevHeight.current;
      // Animate: start offset by the diff, then transition to 0
      el.style.transition = "none";
      el.style.transform = `translateY(${diff}px)`;
      // Force reflow
      el.offsetHeight;
      el.style.transition = "transform 0.1s ease-out";
      el.style.transform = "translateY(0)";
    }
    prevHeight.current = newHeight;
  });

  return (
    <div className="relative overflow-hidden pointer-events-none" style={{ maxHeight: 50 }}>
      <div
        className="absolute inset-x-0 top-0 z-10"
        style={{
          height: "50%",
          background: "linear-gradient(to bottom, rgb(9 9 11) 0%, transparent 100%)",
        }}
      />
      <div key={collapseGen} className="flex flex-col-reverse" style={{ maxHeight: 50 }}>
        <div ref={innerRef}>{children}</div>
      </div>
    </div>
  );
}

/** Renders a list of grouped blocks indented with a left thread line.
 *  When collapsed, shows only the last ~50px with a fade at the top. */
function IndentedChildren({
  groups,
  isError,
  expanded,
}: {
  groups: GroupedBlock[];
  isError?: boolean;
  expanded: boolean;
}) {
  // Increment a generation counter each time we collapse, so the content
  // remounts and all child expanded states reset.
  const [collapseGen, setCollapseGen] = useState(0);
  const prevExpanded = useRef(expanded);
  useEffect(() => {
    if (prevExpanded.current && !expanded) {
      setCollapseGen((g) => g + 1);
    }
    prevExpanded.current = expanded;
  }, [expanded]);

  if (groups.length === 0) return null;

  const content = (
    <div>
      {groups.map((group, i) => {
        const isLast = i === groups.length - 1;
        return (
          <div key={i} className="flex">
            {/* Connector gutter */}
            <div className="relative shrink-0" style={{ width: 16 }}>
              {/* Vertical line — skip for last item, the curve handles it */}
              {!isLast && (
                <div
                  className="absolute left-0 w-0.5 bg-zinc-700"
                  style={{ top: 0, bottom: 0 }}
                />
              )}
              {/* Curve from vertical line into child */}
              <div
                className="absolute left-0 border-l-2 border-b-2 border-zinc-700"
                style={{
                  top: 0,
                  height: "50%",
                  width: 14,
                  borderBottomLeftRadius: 8,
                }}
              />
            </div>
            {/* Child block */}
            <div className="flex-1 min-w-0 py-px">
              <GroupedBlockView
                group={group}
                isError={isError}
                position={
                  groups.length === 1
                    ? "only"
                    : i === 0
                      ? "first"
                      : isLast
                        ? "last"
                        : "middle"
                }
              />
            </div>
          </div>
        );
      })}
    </div>
  );

  return (
    <div className="ml-2 py-1">
      {expanded ? (
        content
      ) : (
        <CollapsedTail collapseGen={collapseGen}>{content}</CollapsedTail>
      )}
    </div>
  );
}

// ─── Agent/Task tool ─────────────────────────────────────────────────────────

function AgentToolView({
  toolUse,
  toolResult,
  position = "only",
}: {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  position?: BlockPosition;
}) {
  const [expanded, setExpanded] = useState(false);
  const tc = toolTextClasses(toolResult);
  const isDone = !!toolResult;
  const isErr = toolResult?.is_error ?? false;
  const name = toolUse?.tool_name ?? "Agent";
  const input = parseInput(toolUse);
  const isAsync = input.run_in_background === true || input.run_in_background === "true";
  const description = (input.description as string) ??
    (input.prompt as string)?.slice(0, 80) ??
    "";
  const childBlocks = toolUse?.children ?? [];
  const childGroups = groupBlocks(childBlocks, true);

  return (
    <div>
      {/* Header bubble */}
      <div
        className={`${toolRadius(position)} text-xs overflow-hidden ${isAsync ? "border-zinc-700 bg-transparent" : toolBorderClass(toolResult)} ${!isDone ? "tool-shimmer" : ""}`}
        style={{ borderWidth: "1px", borderStyle: isAsync ? "dashed" : "solid" }}
      >
        <button
          onClick={() => setExpanded(!expanded)}
          className={`w-full flex items-center gap-2 px-4 py-2 text-left ${tc.hover} transition-colors`}
        >
          <Chevron expanded={expanded} />
          <span className={tc.label}>
            {isAsync
              ? (isDone ? "Background Agent" : "Spawning Background Agent…")
              : toolVerb(name, isDone, isErr)}
          </span>
          {description && (
            <span className={`truncate ${tc.primary}`}>
              {description}
            </span>
          )}
        </button>

        {/* Agent result inside the bubble (skip for async) */}
        {expanded && toolResult && !isAsync && (
          <div className={`border-t ${tc.border}`}>
            <AgentResultContent content={toolResult.content} tc={tc} />
          </div>
        )}

        {/* Streaming indicator */}
        {expanded && !isDone && childGroups.length === 0 && (
          <div className={`border-t ${tc.border} px-4 py-2`}>
            <span className="inline-flex gap-1 text-zinc-500 text-sm">
              <span className="animate-pulse">▍</span>
            </span>
          </div>
        )}
      </div>

      {/* Children rendered outside, indented */}
      <IndentedChildren groups={childGroups} isError={isErr} expanded={expanded} />
    </div>
  );
}

/** Parse and render agent result content blocks, filtering internal metadata. */
// ─── Tool group (consecutive same-name calls) ───────────────────────────────

function ToolGroupView({
  toolName,
  entries,
}: {
  toolName: string;
  entries: ToolCallEntry[];
  position?: BlockPosition;
}) {
  const [expanded, setExpanded] = useState(false);
  const allDone = entries.every((e) => !!e.toolResult);
  const hasError = entries.some((e) => e.toolResult?.is_error);
  const tc = toolTextClasses(hasError ? { is_error: true } as ContentBlock : allDone ? { is_error: false } as ContentBlock : undefined);

  const summary = `${toolVerb(toolName, allDone, hasError)} · ${entries.length}`;

  // Convert entries into GroupedBlock[] so IndentedChildren can render them
  // using the same tool call renderers as everything else.
  const childGroups: GroupedBlock[] = entries.map((e) => ({
    kind: "tool_call" as const,
    toolUse: e.toolUse,
    toolResult: e.toolResult,
  }));

  return (
    <div>
      {/* Summary header — plain inline text, no bubble */}
      <button
        onClick={() => setExpanded(!expanded)}
        className={`flex items-center gap-2 px-1 py-1 text-xs ${tc.hover} rounded transition-colors`}
      >
        <Chevron expanded={expanded} />
        <span className={`${hasError ? "text-red-400" : allDone ? "text-zinc-500" : "text-zinc-400"}`}>
          {summary}
        </span>
      </button>

      {/* Children rendered outside, indented — same as agent */}
      <IndentedChildren groups={childGroups} isError={hasError} expanded={expanded} />
    </div>
  );
}

// ─── Agent result content parser ─────────────────────────────────────────────

function AgentResultContent({
  content,
  tc,
}: {
  content?: string;
  tc: ReturnType<typeof toolTextClasses>;
}) {
  if (!content) return null;

  // Try to parse as JSON array of content blocks.
  let blocks: { type: string; text?: string }[] = [];
  try {
    const parsed = JSON.parse(content);
    if (Array.isArray(parsed)) {
      blocks = parsed;
    } else {
      // Not an array — render as plain text.
      return (
        <div className={`px-4 py-2 whitespace-pre-wrap text-[11px] ${tc.content}`}>
          {content}
        </div>
      );
    }
  } catch {
    // Not JSON — render as plain text.
    return (
      <div className={`px-4 py-2 whitespace-pre-wrap text-[11px] ${tc.content}`}>
        {content}
      </div>
    );
  }

  // Filter out internal metadata blocks (agentId, usage tags, output_file paths).
  const isInternal = (text: string) =>
    /^agentId:|^The agent is working|^Do not duplicate|^output_file:|^If asked, you can check|<usage>/.test(text.trim());

  const userBlocks = blocks.filter(
    (b) => b.type === "text" && b.text && !isInternal(b.text)
  );

  if (userBlocks.length === 0) return null;

  return (
    <div className="px-4 py-2 space-y-2">
      {userBlocks.map((b, i) => (
        <div
          key={i}
          className={`whitespace-pre-wrap text-sm ${tc.content}`}
        >
          {b.text}
        </div>
      ))}
    </div>
  );
}

// ─── Generic fallback ────────────────────────────────────────────────────────

function GenericToolView({
  toolUse,
  toolResult,
  position = "only",
}: {
  toolUse?: ContentBlock;
  toolResult?: ContentBlock;
  position?: BlockPosition;
}) {
  const [expanded, setExpanded] = useState(false);
  const tc = toolTextClasses(toolResult);
  const name = toolUse?.tool_name ?? "tool";
  const isDone = !!toolResult;

  return (
    <div className={`${toolRadius(position)} border text-xs overflow-hidden ${toolBorderClass(toolResult)} ${!isDone ? "tool-shimmer" : ""}`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className={`w-full flex items-center gap-2 px-4 py-2 text-left ${tc.hover} transition-colors`}
      >
        <Chevron expanded={expanded} />
        <span className={`font-mono ${tc.primary}`}>
          {toolVerb(name, isDone, toolResult?.is_error)}
        </span>
      </button>

      {expanded && (
        <div className={`border-t ${tc.border} px-4 py-2 space-y-2`}>
          {toolUse?.input_json && (
            <div>
              <div className={`text-[10px] mb-1 ${tc.secondary}`}>input</div>
              <pre className={`overflow-x-auto max-h-40 overflow-y-auto ${tc.content}`}>
                {toolUse.input_json}
              </pre>
            </div>
          )}
          {toolResult && (
            <div>
              <div className={`text-[10px] mb-1 ${tc.secondary}`}>
                {toolResult.is_error ? "error" : "result"}
              </div>
              <pre className={`overflow-x-auto max-h-48 overflow-y-auto whitespace-pre-wrap ${tc.content}`}>
                {toolResult.content}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Hook approval widget ────────────────────────────────────────────────────

function HookApproval({
  request,
  onResolve,
}: {
  request: HookDecisionRequest;
  onResolve: (requestId: string, allow: boolean) => void;
}) {
  const toolName = request.tool_name ?? "Unknown tool";
  const toolInput = request.tool_input;

  let summary = "";
  if (toolInput) {
    if (toolName === "Bash") {
      summary = (toolInput.command as string) ?? "";
    } else if (toolName === "Read" || toolName === "Edit" || toolName === "Write") {
      const fp = (toolInput.file_path as string) ?? "";
      summary = fp.split(/[/\\]/).pop() ?? fp;
    } else {
      summary = JSON.stringify(toolInput).slice(0, 80);
    }
  }

  return (
    <div className="space-y-2">
      {/* Agent's question — left aligned like an assistant message */}
      <div className="flex justify-start">
        <div className="max-w-[70%] rounded-2xl rounded-tl-sm bg-amber-950/40 border border-amber-800/50 px-4 py-2.5 text-sm">
          <div className="flex items-center gap-2 mb-1">
            <span className="w-1.5 h-1.5 rounded-full bg-amber-400 animate-pulse shrink-0" />
            <span className="text-amber-300 text-xs">
              May I use <span className="font-medium">{toolName}</span>?
            </span>
          </div>
          {summary && (
            <pre className="text-xs text-zinc-400 font-mono whitespace-pre-wrap break-all max-h-24 overflow-y-auto">
              {summary}
            </pre>
          )}
        </div>
      </div>
      {/* User's reply options — right aligned like a user message */}
      <div className="flex justify-end gap-2">
        <button
          onClick={() => onResolve(request.request_id, false)}
          className="rounded-2xl rounded-tr-sm bg-zinc-800/40 border border-dashed border-zinc-600 px-4 py-2 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-300 transition-colors"
        >
          Deny
        </button>
        <button
          onClick={() => onResolve(request.request_id, true)}
          className="rounded-2xl rounded-tr-sm bg-indigo-600/20 border border-dashed border-indigo-500/50 px-4 py-2 text-xs font-medium text-indigo-300 hover:bg-indigo-600/40 hover:text-indigo-200 transition-colors"
        >
          Allow
        </button>
      </div>
    </div>
  );
}
