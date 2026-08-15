import { useEffect, useLayoutEffect, useRef } from "react";
import { pushToast } from "../lib/toasts";
import type { PatchEdge, PatchGraph, PatchNode, Position } from "../lib/types";
import type { Workspace } from "./context";
import { MAX_EDGES, MAX_NODES, newNodeId } from "./graph";
import { isTyping } from "./useHotkeys";

/** Nodes lifted off a patch, with the wires that ran between them. */
export interface Clipboard {
  nodes: readonly PatchNode[];
  edges: readonly PatchEdge[];
}

/** How far a pasted copy lands from what it was copied off, per paste. Enough that the copy is
 * plainly a second face rather than a redraw of the first, and little enough that it stays beside
 * the original instead of somewhere the camera has to be moved to. */
export const PASTE_OFFSET_PX = 32;

/**
 * The nodes these ids name, with the wires running between them.
 *
 * Only the wires with *both* ends in the selection: a wire out of the selection names a node the
 * copy does not carry, and a channel pasted still claiming the original's radio would be a second
 * face on one engine channel.
 */
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

/** Why this clipboard cannot be pasted into this graph, or `null`. The server validates the whole
 * snapshot on every write, so a paste past either bound would be refused along with the next
 * gesture that follows it — it is refused here instead, while there is something to say about it. */
export function pasteRefusal(graph: PatchGraph, clipboard: Clipboard): string | null {
  if (graph.nodes.length + clipboard.nodes.length > MAX_NODES) {
    return `a patch holds ${MAX_NODES} nodes`;
  }
  if ((graph.edges ?? []).length + clipboard.edges.length > MAX_EDGES) {
    return `a patch holds ${MAX_EDGES} wires`;
  }
  return null;
}

/** Fresh ids for a paste of this clipboard, one per node, in its order. */
function pasteIds(clipboard: Clipboard): string[] {
  return clipboard.nodes.map((node) => newNodeId(node.kind));
}

/** The graph with the clipboard added at `offset`, under the ids the caller minted — supplied
 * rather than made here so the pasted nodes can be selected by the same call that stores them. */
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
  // A radio is opened once and binds to one node (`bindDevices`), so the copy of a device names no
  // radio: one that claimed the original's would be a face that can never bind, and its ✕ would
  // offer to close a radio another face is running.
  return node.kind === "device" ? { ...node, id, position, data: {} } : { ...node, id, position };
}

/**
 * Copy and paste on the patch.
 *
 * Canvas-scoped on purpose: what these keys act on is the selection React Flow is drawing, and
 * the rack has no nodes to add. `selection` is that selection; `onPasted` is handed the fresh ids
 * so the canvas can select them once they are drawn.
 */
export function useClipboard(
  workspace: Workspace,
  selection: readonly string[],
  onPasted: (ids: readonly string[]) => void,
): void {
  // The listener is installed once and reads the current gesture through this ref, the same way
  // `useHotkeys` does. Written after commit: a ref written by a render React discards would leave
  // the listener acting on a selection that never existed.
  const latest = useRef({ workspace, selection, onPasted });
  useLayoutEffect(() => {
    latest.current = { workspace, selection, onPasted };
  });
  // How many times this clipboard has been pasted. Each paste steps one offset further, so
  // pasting twice leaves two faces side by side rather than one exactly under the other.
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
        // Text the operator has highlighted — a log row, a decoded callsign — is what they meant
        // to copy, and the browser already does that better than this would.
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

/** One clipboard for the tab, not one per canvas: it outlives the patch/rack switch and a
 * workspace switch, so a chain copied out of one workspace can be pasted into the next. */
let clipboard: Clipboard | null = null;

function hasTextSelection(): boolean {
  const selection = window.getSelection();
  return selection !== null && !selection.isCollapsed && selection.toString() !== "";
}
