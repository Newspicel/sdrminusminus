import {
  Background,
  BackgroundVariant,
  type Edge,
  type Node,
  ReactFlow,
  useEdgesState,
  useNodesState,
  useReactFlow,
} from "@xyflow/react";
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "../components/BaseControls";
import { BTN_QUIET, SURFACE } from "../components/controls";
import type { PatchGraph, PatchNode } from "../lib/types";
import { useClipboard } from "./clipboard";
import { useWorkspaceContext } from "./context";
import {
  edgeKey,
  edgeWarning,
  type GraphContext,
  isPinned,
  isResizable,
  NODE_SIZE,
  nodeOf,
  patchNode,
  pin,
  portOf,
  removeEdge,
  sameGraph,
  unpin,
} from "./graph";
import { useConnections, useGraphChanges } from "./handlers";
import { NODE_TYPES } from "./nodes";
import { focusNode } from "./selection";

export interface FlowData extends Record<string, unknown> {
  node: PatchNode;
}

const FIT_VIEW = { padding: 0.12, maxZoom: 1 } as const;

const DELETE_KEYS = ["Backspace", "Delete"];

export function Canvas() {
  const workspace = useWorkspaceContext();
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<FlowData>>(
    toFlowNodes(workspace.graph),
  );
  const [edges, setEdges, onEdgesChange] = useEdgesState(
    toFlowEdges(workspace.graph, workspace.context),
  );

  const pasted = useRef<ReadonlySet<string>>(new Set());
  const selection = nodes.filter((node) => node.selected).map((node) => node.id);
  useClipboard(
    workspace,
    selection.length > 0 ? selection : workspace.selected === null ? [] : [workspace.selected],
    useCallback((ids: readonly string[]) => {
      pasted.current = new Set(ids);
    }, []),
  );

  const held = useRef<PatchGraph>(workspace.graph);
  const context = workspace.context;
  useEffect(() => {
    if (sameGraph(held.current, workspace.graph)) {
      setEdges(toFlowEdges(workspace.graph, context));
      return;
    }
    held.current = workspace.graph;
    const fresh = pasted.current;
    const arrived = workspace.graph.nodes.some((node) => fresh.has(node.id));
    if (arrived) {
      pasted.current = new Set();
    }
    setNodes((previous) =>
      toFlowNodes(workspace.graph).map((node) => {
        const mounted = previous.find((candidate) => candidate.id === node.id);
        const selected = arrived ? fresh.has(node.id) : (mounted?.selected ?? false);
        return mounted === undefined ? { ...node, selected } : { ...mounted, ...node, selected };
      }),
    );
    setEdges(toFlowEdges(workspace.graph, context));
  }, [workspace.graph, context, setNodes, setEdges]);

  const focus = workspace.selected;
  useEffect(() => {
    setNodes((previous) => focusNode(previous, focus));
  }, [focus, workspace.graph, setNodes]);

  const commitGeometry = useCallback(() => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: {
        ...snapshot.graph,
        nodes: snapshot.graph.nodes.map((node) => {
          const flow = flowRef.current.find((candidate) => candidate.id === node.id);
          if (flow === undefined) {
            return node;
          }
          const natural = NODE_SIZE[node.kind];
          const { width: w, height: h } = flow;
          const resized =
            isResizable(node.kind) &&
            w != null &&
            h != null &&
            (w !== natural.w || h !== natural.h);
          const { size: _dropped, ...rest } = node;
          return {
            ...rest,
            position: { x: flow.position.x, y: flow.position.y },
            ...(resized ? { size: { w, h } } : {}),
          };
        }),
      },
    }));
  }, [workspace]);

  const flowRef = useRef(nodes);
  useLayoutEffect(() => {
    flowRef.current = nodes;
  });

  const { handleNodesChange, handleEdgesChange, onBeforeDelete } = useGraphChanges(
    workspace,
    onNodesChange,
    onEdgesChange,
    commitGeometry,
  );

  const { isValidConnection, onConnect, onConnectEnd } = useConnections(workspace);

  const [menu, setMenu] = useState<Menu | null>(null);
  const openMenu = useCallback((event: React.MouseEvent, target: Menu["target"]) => {
    event.preventDefault();
    setMenu({ x: event.clientX, y: event.clientY, target });
  }, []);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={NODE_TYPES}
        onNodesChange={handleNodesChange}
        onEdgesChange={handleEdgesChange}
        onNodeDragStop={commitGeometry}
        onConnect={onConnect}
        onConnectEnd={onConnectEnd}
        onBeforeDelete={onBeforeDelete}
        isValidConnection={isValidConnection}
        onPaneClick={() => {
          workspace.select(null);
          setMenu(null);
        }}
        onNodeClick={() => setMenu(null)}
        onNodeContextMenu={(event, node) => openMenu(event, { kind: "node", id: node.id })}
        onEdgeContextMenu={(event, edge) => openMenu(event, { kind: "edge", id: edge.id })}
        onPaneContextMenu={(event) => openMenu(event as React.MouseEvent, { kind: "pane" })}
        deleteKeyCode={DELETE_KEYS}
        panOnScroll
        panOnScrollSpeed={1}
        fitView
        fitViewOptions={FIT_VIEW}
        minZoom={0.15}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        className="min-h-0 flex-1 bg-bg"
      >
        <Background variant={BackgroundVariant.Dots} gap={24} size={1} className="!bg-bg" />
      </ReactFlow>
      {menu !== null && <ContextMenu menu={menu} onClose={() => setMenu(null)} />}
    </div>
  );
}

interface Menu {
  x: number;
  y: number;
  target: { kind: "node"; id: string } | { kind: "edge"; id: string } | { kind: "pane" };
}

function ContextMenu({ menu, onClose }: { menu: Menu; onClose: () => void }) {
  const workspace = useWorkspaceContext();
  const { fitView } = useReactFlow();
  const menuRef = useRef<HTMLDivElement>(null);
  const node = menu.target.kind === "node" ? nodeOf(workspace.graph, menu.target.id) : undefined;
  const pinned = node !== undefined && isPinned(workspace.rack, node.id);

  useEffect(() => {
    const dismiss = (event: Event) => {
      if (event instanceof KeyboardEvent) {
        if (event.key === "Escape") {
          onClose();
        }
        return;
      }
      if (event.target instanceof Node && menuRef.current?.contains(event.target) === true) {
        return;
      }
      onClose();
    };
    window.addEventListener("keydown", dismiss);
    window.addEventListener("pointerdown", dismiss, { capture: true });
    return () => {
      window.removeEventListener("keydown", dismiss);
      window.removeEventListener("pointerdown", dismiss, { capture: true });
    };
  }, [onClose]);

  const item = (label: string, act: () => void, danger = false) => (
    <Button
      key={label}
      type="button"
      className={`${BTN_QUIET} w-full justify-start ${danger ? "hover:text-danger" : ""}`}
      onClick={() => {
        act();
        onClose();
      }}
    >
      {label}
    </Button>
  );

  const items: ReactNode[] = [];
  if (node !== undefined) {
    items.push(
      item(pinned ? "Unpin from the rack" : "Pin to the rack", () =>
        workspace.edit((snapshot) => ({
          ...snapshot,
          rack: pinned ? unpin(snapshot.rack ?? {}, node.id) : pin(snapshot.rack ?? {}, node.id),
        })),
      ),
    );
    if (node.size != null && isResizable(node.kind)) {
      items.push(
        item("Reset size", () =>
          workspace.edit((snapshot) => ({
            ...snapshot,
            graph: patchNode(snapshot.graph, node.id, ({ size: _size, ...rest }) => rest),
          })),
        ),
      );
    }
  }
  if (menu.target.kind === "edge") {
    const key = menu.target.id;
    items.push(
      item(
        "Delete wire",
        () =>
          workspace.edit((snapshot) => ({ ...snapshot, graph: removeEdge(snapshot.graph, key) })),
        true,
      ),
    );
  }
  if (menu.target.kind === "pane") {
    items.push(item("Fit the patch on screen", () => void fitView(FIT_VIEW)));
  }

  return (
    <div
      ref={menuRef}
      role="menu"
      className={`${SURFACE} fixed z-40 flex w-52 flex-col p-1`}
      style={{ left: menu.x, top: menu.y }}
    >
      {items}
      {node !== undefined && (
        <span className="px-2 py-1 text-[10px] text-ink-faint">
          Backspace deletes the selection — a node or a wire.
        </span>
      )}
    </div>
  );
}

function toFlowNodes(graph: PatchGraph): Node<FlowData>[] {
  return graph.nodes.map((node) => {
    const size = (isResizable(node.kind) ? node.size : null) ?? NODE_SIZE[node.kind];
    return {
      id: node.id,
      type: node.kind,
      position: node.position,
      data: { node },
      width: size.w,
      height: size.h,
      dragHandle: ".node-drag",
    };
  });
}

function toFlowEdges(graph: PatchGraph, context: GraphContext): Edge[] {
  return (graph.edges ?? []).map((edge) => {
    const warning = edgeWarning(context, graph, edge.from, edge.to);
    const carried = portOf(context, graph, edge.from, "out")?.port_type;
    return {
      id: edgeKey(edge),
      source: edge.from.node,
      sourceHandle: edge.from.port,
      target: edge.to.node,
      targetHandle: edge.to.port,
      className: warning === null ? `wire-${carried}` : "wire-fault",
      ...(warning === null ? {} : { label: warning, labelShowBg: false }),
    };
  });
}
