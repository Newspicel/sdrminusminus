import { useMutation } from "@tanstack/react-query";
import { Handle, NodeResizer, Position } from "@xyflow/react";
import { createContext, type ReactNode, useContext, useRef } from "react";
import { Button } from "../../components/BaseControls";
import { ICON_BTN_SM } from "../../components/controls";
import { PortalContainerProvider } from "../../components/PortalContainer";
import { pushToast } from "../../lib/toasts";
import type { NodeCategory, PatchNode, PortSpec, PortType } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import {
  isPinned,
  isResizable,
  nodeMinSize,
  PORT_STEP_PX,
  PORT_TOP_PX,
  pin,
  portLabel,
  portsOf,
  pruneRack,
  removeNode,
  unpin,
} from "../graph";
import { closeEngineObjects } from "../remove";

const Surface = createContext<"canvas" | "rack">("rack");

export function CanvasSurface({ children }: { children: ReactNode }) {
  return <Surface value="canvas">{children}</Surface>;
}

const Active = createContext(true);

export function useFaceActive(): boolean {
  return useContext(Active);
}

const CATEGORY_STRIP: Record<NodeCategory, string> = {
  source: "bg-cat-source",
  channel: "bg-cat-channel",
  display: "bg-cat-display",
  feature: "bg-cat-feature",
  sink: "bg-cat-sink",
};

const PORT_COLOR: Record<PortType, string> = {
  iq: "text-port-iq",
  baseband: "text-port-baseband",
  audio: "text-port-audio",
  events: "text-port-events",
  video: "text-port-video",
  control: "text-port-control",
  position: "text-accent",
  tx: "text-port-tx",
};

function PortGlyph({ type }: { type: PortType }) {
  const common = {
    fill: type === "tx" ? "none" : "currentColor",
    stroke: type === "tx" ? "currentColor" : "var(--color-line-strong)",
    strokeWidth: 1,
  };
  return (
    <svg aria-hidden viewBox="0 0 12 12" className="pointer-events-none size-3 overflow-visible">
      {type === "iq" || type === "position" || type === "tx" ? (
        <circle cx="6" cy="6" r="4.5" {...common} />
      ) : type === "baseband" ? (
        <path d="M10.5 6 A4.5 4.5 0 0 1 1.5 6 Z" {...common} />
      ) : type === "audio" ? (
        <path d="M6 1 11 6 6 11 1 6Z" {...common} />
      ) : type === "events" ? (
        <path d="M3 1h6l2 5-2 5H3L1 6Z" {...common} />
      ) : type === "video" ? (
        <path d="M6 1 11 11H1Z" {...common} />
      ) : (
        <path d="M1 1 11 6 1 11Z" {...common} />
      )}
    </svg>
  );
}

function PinGlyph({ pinned }: { pinned: boolean }) {
  return (
    <svg aria-hidden viewBox="0 0 12 12" className="pointer-events-none size-3">
      <rect
        x="1.5"
        y="1.5"
        width="9"
        height="9"
        fill="none"
        stroke="currentColor"
        strokeWidth="1"
      />
      {pinned && <rect x="4" y="4" width="4" height="4" fill="currentColor" />}
    </svg>
  );
}

export interface NodeShellProps {
  node: PatchNode;
  title: string;
  category: NodeCategory;
  subtitle?: ReactNode;
  live?: boolean;
  actions?: ReactNode;
  children: ReactNode;
}

export function NodeShell({
  node,
  title,
  category,
  subtitle,
  live = true,
  actions,
  children,
}: NodeShellProps) {
  const workspace = useWorkspaceContext();
  const surface = useContext(Surface);
  const remove = useRemoveNode(node);
  const ports = surface === "canvas" ? portsOf(workspace.context, workspace.graph, node) : [];
  const pinned = isPinned(workspace.rack, node.id);
  const selected = workspace.selected === node.id;
  const active = surface === "rack" || selected;
  const minimum = nodeMinSize(node.kind, ports);
  const portalContainer = useRef<HTMLDivElement>(null);

  return (
    <div
      ref={portalContainer}
      style={surface === "canvas" ? { minHeight: minimum.h } : undefined}
      className={`relative flex h-full min-h-0 w-full flex-col border bg-panel ${
        selected ? "border-accent" : "border-line"
      } ${live ? "" : "opacity-60"}`}
    >
      <PortalContainerProvider container={portalContainer}>
        {surface === "canvas" && isResizable(node.kind) && (
          <NodeResizer
            minWidth={minimum.w}
            minHeight={minimum.h}
            lineClassName="!border-accent/40"
            handleClassName="!size-2 !rounded-none !border-accent !bg-panel"
          />
        )}
        <header
          className={`flex h-6.5 shrink-0 items-center gap-2 border-b border-line bg-panel-2 pr-1 ${
            surface === "canvas" ? "node-drag cursor-grab active:cursor-grabbing" : ""
          }`}
        >
          <span aria-hidden className={`h-full w-1 ${CATEGORY_STRIP[category]}`} />
          <span className="legend truncate text-ink-dim">{node.label ?? title}</span>
          {subtitle !== undefined && (
            <span className="legend ml-auto truncate text-ink-faint">{subtitle}</span>
          )}
          <span
            className={`nodrag flex cursor-auto items-center gap-0.5 ${subtitle === undefined ? "ml-auto" : ""}`}
          >
            {actions}
            <Button
              type="button"
              aria-label={pinned ? "Unpin from the rack" : "Pin to the rack"}
              aria-pressed={pinned}
              title={pinned ? "On the rack — click to take it off" : "Pin to the rack"}
              className={`${ICON_BTN_SM} ${pinned ? "bg-accent/15 text-accent" : "text-ink-faint"}`}
              onClick={() =>
                workspace.edit((snapshot) => ({
                  ...snapshot,
                  rack: pinned
                    ? unpin(snapshot.rack ?? {}, node.id)
                    : pin(snapshot.rack ?? {}, node.id),
                }))
              }
            >
              <PinGlyph pinned={pinned} />
            </Button>
            <Button
              type="button"
              aria-label={`Remove ${node.label ?? title}`}
              title="Remove from the patch"
              className={`${ICON_BTN_SM} text-ink-faint hover:text-danger`}
              onClick={remove}
            >
              ✕
            </Button>
          </span>
        </header>

        <div
          className={`relative flex min-h-0 flex-1 flex-col overflow-hidden nodrag nopan ${
            active ? "nowheel" : ""
          }`}
        >
          <Active value={active}>{children}</Active>
          {!active && (
            <span
              aria-hidden
              className="absolute inset-0 z-20"
              onPointerDown={() => workspace.select(node.id)}
            />
          )}
        </div>

        {ports.map((port, index) => (
          <PortHandle
            key={`${port.direction}:${port.name}`}
            port={port}
            label={portLabel(port.name, ports)}
            offset={PORT_TOP_PX + PORT_STEP_PX * indexOnSide(ports, index)}
          />
        ))}
      </PortalContainerProvider>
    </div>
  );
}

function useRemoveNode(node: PatchNode): () => void {
  const workspace = useWorkspaceContext();
  const drop = useMutation({
    mutationFn: () => closeEngineObjects(workspace, [node.id]),
    onSuccess: () =>
      workspace.edit((snapshot) => {
        const graph = removeNode(snapshot.graph, node.id);
        return { ...snapshot, graph, rack: pruneRack(snapshot.rack ?? {}, graph) };
      }),
    onError: (error: Error) => pushToast(error.message),
  });

  return () => drop.mutate();
}

function indexOnSide(ports: readonly PortSpec[], index: number): number {
  const side = ports[index]?.direction;
  return ports.slice(0, index).filter((port) => port.direction === side).length;
}

function PortHandle({ port, label, offset }: { port: PortSpec; label: string; offset: number }) {
  const out = port.direction === "out";
  const description =
    port.note == null ? `${label} (${port.port_type})` : `${label} — ${port.note}`;
  return (
    <>
      <Handle
        id={port.name}
        type={out ? "source" : "target"}
        position={out ? Position.Right : Position.Left}
        style={{ top: offset }}
        title={description}
        aria-label={`${out ? "output" : "input"} ${description}`}
        className={`!size-3 !border-0 !bg-transparent ${PORT_COLOR[port.port_type]}`}
      >
        <PortGlyph type={port.port_type} />
      </Handle>
      <span
        aria-hidden
        style={{ top: offset }}
        className={`legend pointer-events-none absolute z-10 -translate-y-1/2 rounded-xs bg-bg/85 px-1 whitespace-nowrap select-none text-ink-faint ${
          out ? "left-full ml-2.5" : "right-full mr-2.5"
        }`}
      >
        {label}
      </span>
    </>
  );
}

export function FaceBody({ children, scroll = true }: { children: ReactNode; scroll?: boolean }) {
  return (
    <div
      className={`flex min-h-0 flex-1 flex-col overflow-x-hidden ${scroll ? "overflow-y-auto" : ""}`}
    >
      {children}
    </div>
  );
}

export function FaceEmpty({ children }: { children: ReactNode }) {
  return <p className="p-3 text-sm text-ink-dim">{children}</p>;
}

export function FaceFooter({ children }: { children: ReactNode }) {
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-line p-2">
      {children}
    </div>
  );
}
