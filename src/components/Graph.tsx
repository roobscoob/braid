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
  const hookRequests = useStore((s) => s.hookRequests);
  const pending = useStore((s) => s.pending);

  // Build a set of node ids that have pending hook requests.
  const needsInput = new Set<string>();
  for (const req of hookRequests) {
    if (req.node_id) {
      // node_id is the turn_id — check if there's a pending turn for it.
      const pt = pending.get(req.node_id);
      if (pt) needsInput.add(pt.nodeId);
    }
  }

  const selectedRef = useRef<HTMLDivElement>(null);

  const flat = flattenTree(nodes, rootId);
  const maxRail = flat.reduce((m, f) => Math.max(m, f.rail), 0);
  const graphWidth = (maxRail + 1) * 24 + 16;

  // Build a row-index lookup.
  const rowOf = new Map<string, number>();
  flat.forEach((f, i) => rowOf.set(f.node.id, i));

  // Pre-compute cross-rail edges (child row < parent row since newest is at top).
  interface CrossEdge {
    childRow: number;
    parentRow: number;
    childRail: number;
    parentRail: number;
    color: string;
  }
  const crossEdges: CrossEdge[] = [];
  for (let i = 0; i < flat.length; i++) {
    const f = flat[i];
    if (!f.node.parent_id) continue;
    const parentIdx = rowOf.get(f.node.parent_id);
    if (parentIdx == null) continue;
    const pf = flat[parentIdx];
    if (pf.rail !== f.rail) {
      crossEdges.push({
        childRow: i,
        parentRow: parentIdx,
        childRail: f.rail,
        parentRail: pf.rail,
        color: RAIL_COLORS[f.rail % RAIL_COLORS.length],
      });
    }
  }

  // For each row, gather what to draw:
  // - passThrough: vertical lines for cross-rail edges passing through
  // - curveOut: curves starting at this (parent) row going up to a child rail
  // - arriveFrom: this row is the child end of a cross-rail edge (line from below)
  function edgesForRow(rowIdx: number) {
    const passThrough: { rail: number; color: string }[] = [];
    const curveOut: { toRail: number; color: string }[] = [];
    let arriveFromBelow = false;

    for (const edge of crossEdges) {
      if (rowIdx === edge.parentRow) {
        // This node is the parent — draw curve from our dot up to child's rail
        curveOut.push({ toRail: edge.childRail, color: edge.color });
      } else if (rowIdx === edge.childRow) {
        // This node is the child — draw line from below to dot
        arriveFromBelow = true;
      } else if (rowIdx > edge.childRow && rowIdx < edge.parentRow) {
        // Between child and parent — pass-through on child's rail
        passThrough.push({ rail: edge.childRail, color: edge.color });
      }
    }
    return { passThrough, curveOut, arriveFromBelow };
  }

  // Auto-scroll to the selected node when it changes (deferred to avoid blocking paint).
  useEffect(() => {
    const id = requestAnimationFrame(() => {
      selectedRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    });
    return () => cancelAnimationFrame(id);
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

          const parentFlat = f.node.parent_id
            ? flat[rowOf.get(f.node.parent_id)!]
            : null;
          const parentOnSameRail = parentFlat != null && parentFlat.rail === f.rail;

          const hasChildAbove = f.node.children.some((cid) => {
            const ci = rowOf.get(cid);
            return ci != null && flat[ci].rail === f.rail;
          });

          // Active same-rail pass-throughs (rails with nodes both above and below).
          const activeRails = new Set<number>();
          for (let j = rowIdx + 1; j < flat.length; j++) activeRails.add(flat[j].rail);
          const railsAbove = new Set<number>();
          for (let j = 0; j < rowIdx; j++) railsAbove.add(flat[j].rail);
          for (const r of activeRails) {
            if (!railsAbove.has(r)) activeRails.delete(r);
          }

          const { passThrough, curveOut, arriveFromBelow } = edgesForRow(rowIdx);
          // Merge cross-rail pass-throughs into activeRails set for rendering.
          const allPassThrough = new Map<number, string>();
          for (const r of activeRails) {
            if (r !== f.rail) allPassThrough.set(r, RAIL_COLORS[r % RAIL_COLORS.length]);
          }
          for (const pt of passThrough) {
            if (pt.rail !== f.rail) allPassThrough.set(pt.rail, pt.color);
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
                {/* Pass-through vertical lines */}
                {Array.from(allPassThrough).map(([rail, c]) => {
                  const x = rail * 24 + 12;
                  return (
                    <line
                      key={`rail-${rail}`}
                      x1={x} y1={0} x2={x} y2={32}
                      stroke={c}
                      strokeWidth={2}
                      opacity={0.5}
                    />
                  );
                })}
                {/* Line above dot (to child on same rail) */}
                {hasChildAbove && (
                  <line
                    x1={f.rail * 24 + 12} y1={0}
                    x2={f.rail * 24 + 12} y2={16}
                    stroke={color}
                    strokeWidth={2}
                    opacity={0.6}
                  />
                )}
                {/* Line below dot (to parent on same rail) */}
                {parentOnSameRail && (
                  <line
                    x1={f.rail * 24 + 12} y1={16}
                    x2={f.rail * 24 + 12} y2={32}
                    stroke={color}
                    strokeWidth={2}
                    opacity={0.6}
                  />
                )}
                {/* Cross-rail child arriving: line from below to dot */}
                {arriveFromBelow && !parentOnSameRail && (
                  <line
                    x1={f.rail * 24 + 12} y1={32}
                    x2={f.rail * 24 + 12} y2={16}
                    stroke={color}
                    strokeWidth={2}
                    opacity={0.6}
                  />
                )}
                {/* Branch curves: this node is the parent, curve right then up to child's rail */}
                {curveOut.map((co, i) => {
                  const fromX = f.rail * 24 + 12;
                  const toX = co.toRail * 24 + 12;
                  return (
                    <path
                      key={`curve-${i}`}
                      d={`M ${fromX} 16 C ${toX} 16, ${toX} 16, ${toX} 0`}
                      stroke={co.color}
                      strokeWidth={2}
                      fill="none"
                      opacity={0.6}
                    />
                  );
                })}
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
                className={`text-xs truncate flex-1 ${
                  isSelected
                    ? "text-zinc-100 font-medium"
                    : "text-zinc-400"
                }`}
              >
                {f.node.prompt.slice(0, 50)}
                {f.node.prompt.length > 50 ? "\u2026" : ""}
              </span>
              {/* Status icon */}
              {needsInput.has(f.node.id) ? (
                <span className="shrink-0 text-amber-400 text-xs ml-auto" title="Needs input">●</span>
              ) : f.node.streaming ? (
                <span className="shrink-0 text-indigo-400 text-xs ml-auto animate-pulse" title="Working">●</span>
              ) : !f.node.seen ? (
                <span className="shrink-0 text-emerald-400 text-xs ml-auto" title="New">●</span>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}
