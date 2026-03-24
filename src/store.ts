import { create } from "zustand";
import type { ContentBlock, HookDecisionRequest, TreeNode, TurnEvent } from "./types";

interface PendingTurn {
  turnId: string;
  nodeId: string; // temp node id (= turn_id) until finished
  parentId: string | null;
  prompt: string;
}

interface BraidStore {
  nodes: Map<string, TreeNode>;
  rootId: string | null;
  selectedId: string | null;

  // Track in-flight turns so we can map turn_id → node
  pending: Map<string, PendingTurn>;

  // Events that arrived before beginTurn was called for their turn_id.
  earlyEvents: Map<string, TurnEvent[]>;

  // Hook decisions waiting for user input.
  hookRequests: HookDecisionRequest[];

  addNode: (node: TreeNode) => void;
  select: (id: string) => void;
  setRoot: (id: string) => void;

  /** Register a new turn before events start flowing. */
  beginTurn: (turnId: string, parentId: string | null, prompt: string) => void;

  /** Process a single TurnEvent from the backend. */
  handleEvent: (event: TurnEvent) => void;

  /** Add a hook decision request. */
  addHookRequest: (req: HookDecisionRequest) => void;

  /** Remove a hook decision request (after it's resolved). */
  removeHookRequest: (requestId: string) => void;
}

export const useStore = create<BraidStore>((set, get) => ({
  nodes: new Map(),
  rootId: null,
  selectedId: null,
  pending: new Map(),
  earlyEvents: new Map(),
  hookRequests: [],

  addHookRequest: (req) =>
    set((state) => ({ hookRequests: [...state.hookRequests, req] })),

  removeHookRequest: (requestId) =>
    set((state) => ({
      hookRequests: state.hookRequests.filter((r) => r.request_id !== requestId),
    })),

  addNode: (node) =>
    set((state) => {
      const nodes = new Map(state.nodes);
      nodes.set(node.id, node);
      if (node.parent_id) {
        const parent = nodes.get(node.parent_id);
        if (parent && !parent.children.includes(node.id)) {
          nodes.set(node.parent_id, {
            ...parent,
            children: [...parent.children, node.id],
          });
        }
      }
      return { nodes, selectedId: node.id };
    }),

  select: (id) => set({ selectedId: id }),
  setRoot: (id) => set({ rootId: id }),

  beginTurn: (turnId, parentId, prompt) => {
    const pending = new Map(get().pending);
    pending.set(turnId, { turnId, nodeId: turnId, parentId, prompt });

    // Create a placeholder node immediately so the user sees their message.
    const node: TreeNode = {
      id: turnId,
      session_id: "",
      prompt,
      blocks: [],
      model: "",
      cost_usd: 0,
      parent_id: parentId,
      children: [],
      is_error: false,
      streaming: true,
      createdAt: Date.now(),
    };

    const nodes = new Map(get().nodes);
    nodes.set(node.id, node);
    if (parentId) {
      const parent = nodes.get(parentId);
      if (parent && !parent.children.includes(node.id)) {
        nodes.set(parentId, {
          ...parent,
          children: [...parent.children, node.id],
        });
      }
    }

    set({
      pending,
      nodes,
      selectedId: turnId,
      rootId: get().rootId ?? turnId,
    });

    // Flush any events that arrived before this turn was registered.
    const early = get().earlyEvents.get(turnId);
    if (early) {
      const updated = new Map(get().earlyEvents);
      updated.delete(turnId);
      set({ earlyEvents: updated });
      for (const evt of early) {
        get().handleEvent(evt);
      }
    }
  },

  handleEvent: (event) => {
    const { pending, nodes } = get();
    const pt = pending.get(event.turn_id);
    if (!pt) {
      // Turn not registered yet — queue for later.
      const early = new Map(get().earlyEvents);
      const queue = early.get(event.turn_id) ?? [];
      queue.push(event);
      early.set(event.turn_id, queue);
      set({ earlyEvents: early });
      return;
    }

    const nodeId = pt.nodeId;
    const node = nodes.get(nodeId);
    if (!node) return;

    const updated = new Map(nodes);

    // Helper: find the target block list — either top-level or nested
    // inside a parent tool_use's children array.
    function getTargetBlocks(
      blocks: ContentBlock[],
      parentId: string | null | undefined
    ): { blocks: ContentBlock[]; path: number[] } {
      if (!parentId) return { blocks, path: [] };
      // Find the tool_use block with this id and return its children.
      for (let i = 0; i < blocks.length; i++) {
        if (blocks[i].type === "tool_use" && blocks[i].tool_id === parentId) {
          return { blocks: blocks[i].children ?? [], path: [i] };
        }
      }
      // Not found — fall back to top level.
      return { blocks, path: [] };
    }

    // Helper: immutably update blocks at a path.
    function setChildBlocks(
      topBlocks: ContentBlock[],
      path: number[],
      childBlocks: ContentBlock[]
    ): ContentBlock[] {
      if (path.length === 0) return childBlocks;
      const result = [...topBlocks];
      const idx = path[0];
      result[idx] = { ...result[idx], children: childBlocks };
      return result;
    }

    switch (event.type) {
      case "system_init": {
        updated.set(nodeId, {
          ...node,
          session_id: event.session_id,
          model: event.model,
        });
        break;
      }

      case "text_delta": {
        const topBlocks = [...node.blocks];
        const parentId = event.parent_tool_use_id;
        const { blocks: target, path } = getTargetBlocks(topBlocks, parentId);
        const targetBlocks = [...target];
        const last = targetBlocks[targetBlocks.length - 1];
        if (last && last.type === "text") {
          targetBlocks[targetBlocks.length - 1] = {
            ...last,
            text: (last.text ?? "") + event.text,
          };
        } else {
          targetBlocks.push({ type: "text", text: event.text });
        }
        updated.set(nodeId, { ...node, blocks: setChildBlocks(topBlocks, path, targetBlocks) });
        break;
      }

      case "thinking_delta": {
        const topBlocks = [...node.blocks];
        const parentId = event.parent_tool_use_id;
        const { blocks: target, path } = getTargetBlocks(topBlocks, parentId);
        const targetBlocks = [...target];
        const last = targetBlocks[targetBlocks.length - 1];
        if (last && last.type === "thinking") {
          targetBlocks[targetBlocks.length - 1] = {
            ...last,
            text: (last.text ?? "") + event.text,
          };
        } else {
          targetBlocks.push({ type: "thinking", text: event.text });
        }
        updated.set(nodeId, { ...node, blocks: setChildBlocks(topBlocks, path, targetBlocks) });
        break;
      }

      case "tool_use_start": {
        const topBlocks = [...node.blocks];
        const parentId = event.parent_tool_use_id;
        const { blocks: target, path } = getTargetBlocks(topBlocks, parentId);
        const targetBlocks = [...target];
        targetBlocks.push({
          type: "tool_use",
          tool_name: event.tool_name,
          tool_id: event.tool_id,
          input_json: "",
          parent_tool_use_id: parentId,
          children: [],
        });
        updated.set(nodeId, { ...node, blocks: setChildBlocks(topBlocks, path, targetBlocks) });
        break;
      }

      case "tool_use_input_delta": {
        const blocks = [...node.blocks];
        const deltaToolId = event.tool_id;
        const deltaJson = event.partial_json;
        // Find the matching tool_use — could be top-level or nested.
        const applyDelta = (blks: ContentBlock[]): ContentBlock[] => {
          const result = [...blks];
          for (let i = result.length - 1; i >= 0; i--) {
            if (result[i].type === "tool_use" && (!deltaToolId || result[i].tool_id === deltaToolId)) {
              result[i] = {
                ...result[i],
                input_json: (result[i].input_json ?? "") + deltaJson,
              };
              return result;
            }
            // Check nested children.
            if (result[i].children && result[i].children!.length > 0) {
              const childResult = applyDelta(result[i].children!);
              if (childResult !== result[i].children) {
                result[i] = { ...result[i], children: childResult };
                return result;
              }
            }
          }
          // Fallback: find any last tool_use.
          for (let i = result.length - 1; i >= 0; i--) {
            if (result[i].type === "tool_use") {
              result[i] = {
                ...result[i],
                input_json: (result[i].input_json ?? "") + deltaJson,
              };
              return result;
            }
          }
          return result;
        };
        updated.set(nodeId, { ...node, blocks: applyDelta(blocks) });
        break;
      }

      case "tool_result": {
        const topBlocks = [...node.blocks];
        const parentId = event.parent_tool_use_id;
        const { blocks: target, path } = getTargetBlocks(topBlocks, parentId);
        const targetBlocks = [...target];
        targetBlocks.push({
          type: "tool_result",
          tool_id: event.tool_id,
          content: event.content,
          is_error: event.is_error,
          parent_tool_use_id: parentId,
        });
        updated.set(nodeId, { ...node, blocks: setChildBlocks(topBlocks, path, targetBlocks) });
        break;
      }

      case "agent_progress": {
        // Background agent progress — add a mini status line to the
        // agent's children array.
        const topBlocks = [...node.blocks];
        const agentId = event.agent_tool_id;
        const { blocks: target, path } = getTargetBlocks(topBlocks, agentId);
        const targetBlocks = [...target];
        targetBlocks.push({
          type: "progress",
          tool_name: event.tool_name,
          text: event.description,
        });
        updated.set(nodeId, { ...node, blocks: setChildBlocks(topBlocks, path, targetBlocks) });
        break;
      }

      case "finished": {
        // Swap temp node id → real message_id if they differ.
        const realId = event.message_id;
        // If the turn ended with no content blocks, surface the result text.
        const blocks = [...node.blocks];
        if (blocks.length === 0 && event.result_text) {
          blocks.push({
            type: "text",
            text: event.is_error
              ? `Error: ${event.result_text}`
              : event.result_text,
          });
        }
        const finalNode: TreeNode = {
          ...node,
          blocks,
          id: realId,
          session_id: event.session_id,
          model: event.model,
          cost_usd: event.cost_usd,
          is_error: event.is_error,
          streaming: false,
          commit_sha: event.commit_sha ?? undefined,
        };

        if (realId !== nodeId) {
          // Replace the temp node with the real one.
          updated.delete(nodeId);
          updated.set(realId, finalNode);
          // Update parent's children array.
          if (node.parent_id) {
            const parent = updated.get(node.parent_id);
            if (parent) {
              updated.set(node.parent_id, {
                ...parent,
                children: parent.children.map((c) =>
                  c === nodeId ? realId : c
                ),
              });
            }
          }
        } else {
          updated.set(realId, finalNode);
        }

        const newPending = new Map(pending);
        newPending.delete(event.turn_id);

        set({
          nodes: updated,
          pending: newPending,
          selectedId: realId,
          rootId: get().rootId === nodeId ? realId : get().rootId,
        });
        return;
      }

      case "resume": {
        const blocks = [...node.blocks];
        blocks.push({ type: "separator" });
        updated.set(nodeId, { ...node, blocks });
        break;
      }

      case "error": {
        const blocks = [...node.blocks];
        blocks.push({ type: "text", text: `\n\nError: ${event.message}` });
        updated.set(nodeId, { ...node, blocks, is_error: true, streaming: false });

        const newPending = new Map(pending);
        newPending.delete(event.turn_id);
        set({ nodes: updated, pending: newPending });
        return;
      }

      default:
        return;
    }

    set({ nodes: updated });
  },
}));

/** Compute the chain from root to a given node. */
export function computeChain(
  nodes: Map<string, TreeNode>,
  selectedId: string | null
): TreeNode[] {
  if (!selectedId) return [];
  const chain: TreeNode[] = [];
  let current: TreeNode | undefined = nodes.get(selectedId);
  while (current) {
    chain.unshift(current);
    current = current.parent_id ? nodes.get(current.parent_id) : undefined;
  }
  return chain;
}
