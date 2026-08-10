// The chrome every node face sits in (CANVAS §6): a category strip, a title row, the ports, and
// the face itself. Faces render their instrument and nothing else — the shell owns identity,
// wiring and the pin.
//
// Hue carries the port's data type and only that, and every colour is paired with a shape, so
// the graph reads for a colourblind operator by marker alone (DESIGN.md §3).
import { Handle, NodeResizer, Position } from "@xyflow/react";
import { createContext, type ReactNode, useContext } from "react";
import { ICON_BTN } from "../../components/controls";
import type { NodeCategory, PatchNode, PortSpec, PortType } from "../../lib/types";
import { useStationContext } from "../context";
import { isPinned, pin, portsOf, pruneRack, removeNode, unpin } from "../graph";

/**
 * Where a face is being rendered. Ports and the resizer are React Flow parts and throw outside
 * its provider, and the rack has neither — it is a grid with no wires (CANVAS §5) — so the
 * surface is what decides whether the shell draws them.
 */
const Surface = createContext<"canvas" | "rack">("rack");

export function CanvasSurface({ children }: { children: ReactNode }) {
  return <Surface value="canvas">{children}</Surface>;
}

/** Vertical space the header takes, so ports can be spread down the body only. */
const HEADER_PX = 26;
/** Distance between stacked ports on one side. */
const PORT_STEP_PX = 22;

const CATEGORY_STRIP: Record<NodeCategory, string> = {
  source: "bg-cat-source",
  channel: "bg-cat-channel",
  display: "bg-cat-display",
  feature: "bg-cat-feature",
  sink: "bg-cat-sink",
};

const PORT_SHAPE: Record<PortType, string> = {
  iq: "rounded-full",
  audio: "rotate-45",
  events: "rounded-[1px]",
};

const PORT_COLOR: Record<PortType, string> = {
  iq: "bg-port-iq",
  audio: "bg-port-audio",
  events: "bg-port-events",
};

export interface NodeShellProps {
  node: PatchNode;
  /** Default caption for the kind; the node's own label wins. */
  title: string;
  category: NodeCategory;
  /** One short line under the title — what this node is bound to, or why it is not. */
  subtitle?: ReactNode;
  /** `false` renders the node dimmed: named a radio that is not attached, or waiting for one. */
  live?: boolean;
  /** Right-aligned controls in the header, before the shell's own pin and remove. */
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
  const station = useStationContext();
  const surface = useContext(Surface);
  const ports = surface === "canvas" ? portsOf(station.context, node) : [];
  const pinned = isPinned(station.rack, node.id);
  const selected = station.selected === node.id;

  return (
    <div
      className={`flex h-full min-h-0 w-full flex-col overflow-hidden border bg-panel ${
        selected ? "border-accent" : "border-line"
      } ${live ? "" : "opacity-60"}`}
    >
      {surface === "canvas" && (
        <NodeResizer
          minWidth={220}
          minHeight={140}
          lineClassName="!border-accent/40"
          handleClassName="!size-2 !rounded-none !border-accent !bg-panel"
        />
      )}
      <header className="flex h-6.5 shrink-0 items-center gap-2 border-b border-line bg-panel-2 pr-1">
        <span aria-hidden className={`h-full w-1 ${CATEGORY_STRIP[category]}`} />
        <span className="legend truncate text-ink-dim">{node.label ?? title}</span>
        {subtitle !== undefined && (
          <span className="legend ml-auto truncate text-ink-faint">{subtitle}</span>
        )}
        <span className={`flex items-center gap-0.5 ${subtitle === undefined ? "ml-auto" : ""}`}>
          {actions}
          <button
            type="button"
            aria-label={pinned ? "Unpin from the rack" : "Pin to the rack"}
            aria-pressed={pinned}
            title={pinned ? "Unpin from the rack" : "Pin to the rack"}
            className={`${ICON_BTN} size-5 ${pinned ? "text-accent" : "text-ink-faint"}`}
            onClick={() =>
              station.edit((snapshot) => ({
                ...snapshot,
                rack: pinned
                  ? unpin(snapshot.rack ?? {}, node.id)
                  : pin(snapshot.rack ?? {}, node.id),
              }))
            }
          >
            ▣
          </button>
          <button
            type="button"
            aria-label={`Remove ${node.label ?? title}`}
            title="Remove from the patch"
            className={`${ICON_BTN} size-5 text-ink-faint hover:text-danger`}
            onClick={() =>
              station.edit((snapshot) => {
                const graph = removeNode(snapshot.graph, node.id);
                return { ...snapshot, graph, rack: pruneRack(snapshot.rack ?? {}, graph) };
              })
            }
          >
            ✕
          </button>
        </span>
      </header>

      {/* React Flow claims pointer drags and wheel gestures anywhere on a node unless a subtree
          opts out: without `nodrag nowheel`, dragging a gain slider drags the node and scrolling
          a digit zooms the canvas instead of tuning. The header keeps both, so the node is
          dragged by its title bar — the patch-editor convention. */}
      <div className="nodrag nopan nowheel flex min-h-0 flex-1 flex-col overflow-hidden">
        {children}
      </div>

      {ports.map((port, index) => (
        <PortHandle
          key={`${port.direction}:${port.name}`}
          port={port}
          offset={HEADER_PX + PORT_STEP_PX * indexOnSide(ports, index)}
        />
      ))}
    </div>
  );
}

/** Position among the ports on the same side, so the two sides stack independently. */
function indexOnSide(ports: readonly PortSpec[], index: number): number {
  const side = ports[index]?.direction;
  return ports.slice(0, index).filter((port) => port.direction === side).length;
}

function PortHandle({ port, offset }: { port: PortSpec; offset: number }) {
  const out = port.direction === "out";
  return (
    <Handle
      id={port.name}
      type={out ? "source" : "target"}
      position={out ? Position.Right : Position.Left}
      style={{ top: offset }}
      // The label is the accessible name and the hover title: hue alone never says what a wire
      // carries (DESIGN.md §3).
      title={`${port.name} (${port.port_type})`}
      className={`!size-2.5 !border !border-line-strong ${PORT_COLOR[port.port_type]} ${
        PORT_SHAPE[port.port_type]
      }`}
    />
  );
}

/** The body wrapper a face uses when its content scrolls. React Flow gives a node no scroll
 * container, exactly as a dock panel did not. */
export function FaceBody({ children, scroll = true }: { children: ReactNode; scroll?: boolean }) {
  return (
    <div className={`flex min-h-0 flex-1 flex-col ${scroll ? "overflow-y-auto" : ""}`}>
      {children}
    </div>
  );
}

/** What a face shows instead of its instrument when there is nothing behind it yet. */
export function FaceEmpty({ children }: { children: ReactNode }) {
  return <p className="p-3 text-sm text-ink-dim">{children}</p>;
}
