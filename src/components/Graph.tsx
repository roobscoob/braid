import { useEffect, useRef } from "react";
import { useStore } from "../store";
import type { TreeNode } from "../types";

// Colors for branch rails (cycle through these)
const RAIL_COLORS = [
  "#6366f1", // indigo
  "#f59e0b", // amber
  "#10b981", // emerald
  "#ef4444", // red
  "#8b5cf6", // violet
  "#06b6d4", // cyan
  "#f97316", // orange
  "#ec4899", // pink
];

interface FlatNode {
  node: TreeNode;
  rail: number; // which column/rail this node sits on
  depth: number;
}

/**
 * Flatten the tree into rows ordered **newest-first** (`--date-order`).
 *
 * Every node is sorted by `createdAt` descending.  Rails are assigned by
 * tracing each node's ancestry: the newest child of a parent inherits the
 * parent's rail, older siblings branch off to new rails.
 */
function flattenTree(
  nodes: Map<string, TreeNode>,
  rootId: string | null
): FlatNode[] {
  if (!rootId) return [];

  // Collect all nodes reachable from root.
  const all: TreeNode[] = [];
  function collect(id: string) {
    const node = nodes.get(id);
    if (!node) return;
    all.push(node);
    for (const cid of node.children) collect(cid);
  }
  collect(rootId);

  // Sort newest-first.
  all.sort((a, b) => b.createdAt - a.createdAt);

  // Compute the max createdAt in each node's subtree (including itself).
  // This tells us how "recent" a branch is.
  const maxTime = new Map<string, number>();
  function computeMaxTime(id: string): number {
    const node = nodes.get(id);
    if (!node) return 0;
    let t = node.createdAt;
    for (const cid of node.children) {
      t = Math.max(t, computeMaxTime(cid));
    }
    maxTime.set(id, t);
    return t;
  }
  computeMaxTime(rootId);

  // Group children by parent.
  const childrenByParent = new Map<string, TreeNode[]>();
  for (const node of all) {
    if (node.parent_id == null) continue;
    let arr = childrenByParent.get(node.parent_id);
    if (!arr) {
      arr = [];
      childrenByParent.set(node.parent_id, arr);
    }
    arr.push(node);
  }

  // Assign rails by BFS from root.  At each fork, the child whose subtree
  // has the most recent activity inherits the parent's rail.  Remaining
  // siblings get fresh rails, ordered by their subtree recency (most
  // recent = lowest rail number = leftmost).
  const railOf = new Map<string, number>();
  let nextRail = 0;
  railOf.set(rootId, nextRail++);

  const queue = [rootId];
  while (queue.length > 0) {
    const id = queue.shift()!;
    const children = childrenByParent.get(id) ?? [];
    // Sort by most recent subtree activity first.
    children.sort((a, b) => (maxTime.get(b.id) ?? 0) - (maxTime.get(a.id) ?? 0));
    for (let i = 0; i < children.length; i++) {
      const child = children[i];
      if (i === 0) {
        // Most recently active child inherits parent's rail.
        railOf.set(child.id, railOf.get(id)!);
      } else {
        railOf.set(child.id, nextRail++);
      }
      queue.push(child.id);
    }
  }

  // Compact & reorder rails so the most recently active branch is leftmost.
  // Collect each rail's max subtree time, sort by that descending.
  const railMaxTime = new Map<number, number>();
  for (const [nodeId, rail] of railOf) {
    const t = maxTime.get(nodeId) ?? 0;
    railMaxTime.set(rail, Math.max(railMaxTime.get(rail) ?? 0, t));
  }
  const railsByRecency = [...railMaxTime.entries()]
    .sort((a, b) => b[1] - a[1]);
  const compact = new Map<number, number>();
  railsByRecency.forEach(([rail], i) => compact.set(rail, i));

  // Build the flat list in date order (newest first).
  return all.map((node) => ({
    node,
    rail: compact.get(railOf.get(node.id)!)!,
    depth: 0, // not used for rendering
  }));
}

export function Graph() {
  const nodes = useStore((s) => s.nodes);
  const rootId = useStore((s) => s.rootId);
  const selectedId = useStore((s) => s.selectedId);
  const select = useStore((s) => s.select);

  const selectedRef = useRef<HTMLDivElement>(null);

  const flat = flattenTree(nodes, rootId);
  const maxRail = flat.reduce((m, f) => Math.max(m, f.rail), 0);
  const graphWidth = (maxRail + 1) * 24 + 16;

  // Auto-scroll to the selected node when it changes.
  useEffect(() => {
    selectedRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, [selectedId]);

  return (
    <div className="h-full overflow-y-auto bg-zinc-900 border-r border-zinc-700 select-none">
      <div className="p-3 text-xs font-semibold text-zinc-400 uppercase tracking-wider border-b border-zinc-700">
        Branches
      </div>
      <div className="py-1">
        {flat.map((f, rowIdx) => {
          const isSelected = f.node.id === selectedId;
          const color = RAIL_COLORS[f.rail % RAIL_COLORS.length];

          // Parent is on the same rail = draw a straight line down from dot.
          // If parent is on a different rail, the curve handles it — no tail.
          const parentFlat = f.node.parent_id
            ? flat.find((x) => x.node.id === f.node.parent_id)
            : null;
          const parentOnSameRail = parentFlat != null && parentFlat.rail === f.rail;

          // Child on the same rail = draw a straight line up from dot.
          const hasChildAbove = f.node.children.some((cid) => {
            const child = flat.find((x) => x.node.id === cid);
            return child && child.rail === f.rail;
          });

          // Active rails: rails that have nodes both above and below this row.
          const activeRails = new Set<number>();
          for (let j = rowIdx + 1; j < flat.length; j++) {
            activeRails.add(flat[j].rail);
          }
          // Only keep rails that also appear above us.
          const railsAbove = new Set<number>();
          for (let j = 0; j < rowIdx; j++) {
            railsAbove.add(flat[j].rail);
          }
          for (const r of activeRails) {
            if (!railsAbove.has(r)) activeRails.delete(r);
          }

          return (
            <div
              key={f.node.id}
              ref={isSelected ? selectedRef : undefined}
              className={`flex items-center cursor-pointer hover:bg-zinc-800 transition-colors ${
                isSelected ? "bg-zinc-800" : ""
              }`}
              style={{ paddingLeft: 8, paddingRight: 12, height: 32 }}
              onClick={() => select(f.node.id)}
            >
              {/* Graph rails */}
              <svg
                width={graphWidth}
                height={32}
                className="shrink-0"
              >
                {/* Pass-through vertical lines for active rails */}
                {Array.from(activeRails).map((rail) => {
                  if (rail === f.rail) return null;
                  const x = rail * 24 + 12;
                  return (
                    <line
                      key={`rail-${rail}`}
                      x1={x} y1={0} x2={x} y2={32}
                      stroke={RAIL_COLORS[rail % RAIL_COLORS.length]}
                      strokeWidth={2}
                      opacity={0.5}
                    />
                  );
                })}
                {/* Line above dot (to child) */}
                {hasChildAbove && (
                  <line
                    x1={f.rail * 24 + 12} y1={0}
                    x2={f.rail * 24 + 12} y2={16}
                    stroke={color}
                    strokeWidth={2}
                    opacity={0.6}
                  />
                )}
                {/* Line below dot (to parent on same rail only) */}
                {parentOnSameRail && (
                  <line
                    x1={f.rail * 24 + 12} y1={16}
                    x2={f.rail * 24 + 12} y2={32}
                    stroke={color}
                    strokeWidth={2}
                    opacity={0.6}
                  />
                )}
                {/* Branch curve from parent's rail (below) to this rail */}
                {parentFlat && (() => {
                  if (parentFlat.rail !== f.rail) {
                    const px = parentFlat.rail * 24 + 12;
                    const cx = f.rail * 24 + 12;
                    // Curve from parent rail at bottom up to this rail at dot
                    return (
                      <path
                        d={`M ${px} 32 C ${px} 20, ${cx} 28, ${cx} 16`}
                        stroke={color}
                        strokeWidth={2}
                        fill="none"
                        opacity={0.6}
                      />
                    );
                  }
                  return null;
                })()}
                {/* Dot */}
                <circle
                  cx={f.rail * 24 + 12}
                  cy={16}
                  r={isSelected ? 5 : 4}
                  fill={isSelected ? color : "transparent"}
                  stroke={color}
                  strokeWidth={2}
                />
              </svg>
              {/* Label */}
              <span
                className={`text-xs truncate ${
                  isSelected
                    ? "text-zinc-100 font-medium"
                    : "text-zinc-400"
                }`}
              >
                {f.node.prompt.slice(0, 50)}
                {f.node.prompt.length > 50 ? "\u2026" : ""}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
