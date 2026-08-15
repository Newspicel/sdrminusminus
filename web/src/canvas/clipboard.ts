import { useEffect, useLayoutEffect, useRef } from "react";
import { pushToast } from "../lib/toasts";
import type { PatchEdge, PatchGraph, PatchNode, Position } from "../lib/types";
import type { Workspace } from "./context";
import { MAX_EDGES, MAX_NODES, newNodeId } from "./graph";
import { isTyping } from "./useHotkeys";

export interface Clipboard {
  nodes: readonly PatchNode[];
  edges: readonly PatchEdge[];
}

export const PASTE_OFFSET_PX = 32;

export function copyNodes(graph: PatchGraph, ids: readonly string[]): Clipboard | null {
  const wanted = new Set(ids);
  const nodes = graph.nodes.filter((node) => wanted.has(node.id));
  if (nodes.length === 0) {
    return null;
  }
  const inside = new Set(nodes.map((node) => node.id));
  const edges = (graph.edges ?? []).filter(
    (edge) => inside.has(edge.from.node) && inside.has(edge.to.node),
  );
  return { nodes, edges };
}

export function pasteRefusal(graph: PatchGraph, clipboard: Clipboard): string | null {
  if (graph.nodes.length + clipboard.nodes.length > MAX_NODES) {
    return `a patch holds ${MAX_NODES} nodes`;
  }
  if ((graph.edges ?? []).length + clipboard.edges.length > MAX_EDGES) {
    return `a patch holds ${MAX_EDGES} wires`;
  }
  return null;
}

function pasteIds(clipboard: Clipboard): string[] {
  return clipboard.nodes.map((node) => newNodeId(node.kind));
}

export function pasteNodes(
  graph: PatchGraph,
  clipboard: Clipboard,
  offset: Position,
  ids: readonly string[],
): PatchGraph {
  const minted = new Map(
    clipboard.nodes.map((node, index) => [node.id, ids[index] ?? newNodeId(node.kind)]),
  );
  const rename = (node: string): string => minted.get(node) ?? node;
  return {
    nodes: [
      ...graph.nodes,
      ...clipboard.nodes.map((node) => copyOf(node, rename(node.id), offset)),
    ],
    edges: [
      ...(graph.edges ?? []),
      ...clipboard.edges.map((edge) => ({
        from: { node: rename(edge.from.node), port: edge.from.port },
        to: { node: rename(edge.to.node), port: edge.to.port },
      })),
    ],
  };
}

function copyOf(node: PatchNode, id: string, offset: Position): PatchNode {
  const position = { x: node.position.x + offset.x, y: node.position.y + offset.y };
  return node.kind === "device" ? { ...node, id, position, data: {} } : { ...node, id, position };
}

export function useClipboard(
  workspace: Workspace,
  selection: readonly string[],
  onPasted: (ids: readonly string[]) => void,
): void {
  const latest = useRef({ workspace, selection, onPasted });
  useLayoutEffect(() => {
    latest.current = { workspace, selection, onPasted };
  });
  const pastes = useRef(0);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) {
        return;
      }
      const key = event.key.toLowerCase();
      if ((key !== "c" && key !== "v") || isTyping(event.target)) {
        return;
      }
      const { workspace: active, selection: selected, onPasted: pasted } = latest.current;
      if (key === "c") {
        if (hasTextSelection()) {
          return;
        }
        const copied = copyNodes(active.graph, selected);
        if (copied === null) {
          return;
        }
        clipboard = copied;
        pastes.current = 0;
        pushToast(
          copied.nodes.length === 1 ? "Copied 1 node" : `Copied ${copied.nodes.length} nodes`,
          "info",
        );
        event.preventDefault();
        return;
      }
      const held = clipboard;
      if (held === null) {
        return;
      }
      event.preventDefault();
      const refusal = pasteRefusal(active.graph, held);
      if (refusal !== null) {
        pushToast(refusal);
        return;
      }
      pastes.current += 1;
      const step = PASTE_OFFSET_PX * pastes.current;
      const ids = pasteIds(held);
      active.edit((snapshot) => ({
        ...snapshot,
        graph: pasteNodes(snapshot.graph, held, { x: step, y: step }, ids),
      }));
      pasted(ids);
      active.select(ids[0] ?? null);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
}

let clipboard: Clipboard | null = null;

function hasTextSelection(): boolean {
  const selection = window.getSelection();
  return selection !== null && !selection.isCollapsed && selection.toString() !== "";
}
