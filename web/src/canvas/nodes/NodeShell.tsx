// The chrome every node face sits in (CANVAS §6): a category strip, a title row, the ports, and
// the face itself. Faces render their instrument and nothing else — the shell owns identity,
// wiring and the pin.
//
// Hue carries the port's data type and only that, and every colour is paired with a shape, so
// the graph reads for a colourblind operator by marker alone (DESIGN.md §2).
import { useMutation } from "@tanstack/react-query";
import { Handle, NodeResizer, Position } from "@xyflow/react";
import { createContext, type ReactNode, useContext } from "react";
import { ICON_BTN } from "../../components/controls";
import { pushToast } from "../../lib/toasts";
import type { NodeCategory, PatchNode, PortSpec, PortType } from "../../lib/types";
import { useStationContext } from "../context";
import { isPinned, NODE_MIN_SIZE, pin, portsOf, pruneRack, removeNode, unpin } from "../graph";
import { closeEngineObjects } from "../remove";

/**
 * Where a face is being rendered. Ports and the resizer are React Flow parts and throw outside
 * its provider, and the rack has neither — it is a grid with no wires (CANVAS §5) — so the
 * surface is what decides whether the shell draws them.
 */
const Surface = createContext<"canvas" | "rack">("rack");

export function CanvasSurface({ children }: { children: ReactNode }) {
  return <Surface value="canvas">{children}</Surface>;
}

/**
 * Whether this face owns the pointer and the wheel, or the camera does — a window has to be
 * clicked before its controls answer, which is the rule the desktop already taught everyone.
 *
 * The camera keeps the wheel over every *other* face, so scrolling across the patch is never
 * blocked by whatever the pointer happens to be over; a click makes a face the active one and
 * hands it its own gestures (the dial's wheel, the plot's zoom, the map's pan). Instruments read
 * this to stay inert until then: without it a wheel over an unselected scope would zoom the
 * spectrum *and* pan the patch at once.
 *
 * The rack has no camera, so a face there is always active (CANVAS §5).
 */
const Active = createContext(true);

export function useFaceActive(): boolean {
  return useContext(Active);
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
  // An arrowhead, because control is the one wire that carries an instruction rather than a
  // stream: it points at the radio it drives.
  control: "[clip-path:polygon(0_0,100%_50%,0_100%)]",
  // The same substance as `iq` going the other way, so the same circle — left hollow, because
  // nothing fills it yet (PLAN §12a).
  tx: "rounded-full",
};

/** Fill and edge per type. React Flow's base stylesheet sets the handle's border and size, which
 * is why those two are forced and the fill is not. */
const PORT_PAINT: Record<PortType, string> = {
  iq: "!border !border-line-strong bg-port-iq",
  audio: "!border !border-line-strong bg-port-audio",
  events: "!border !border-line-strong bg-port-events",
  control: "!border !border-line-strong bg-port-control",
  tx: "!border-2 !border-port-tx bg-transparent",
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
  const remove = useRemoveNode(node);
  const ports = surface === "canvas" ? portsOf(station.context, node) : [];
  const pinned = isPinned(station.rack, node.id);
  const selected = station.selected === node.id;
  const active = surface === "rack" || selected;
  const minimum = NODE_MIN_SIZE[node.kind];

  return (
    <div
      className={`flex h-full min-h-0 w-full flex-col overflow-hidden border bg-panel ${
        selected ? "border-accent" : "border-line"
      } ${live ? "" : "opacity-60"}`}
    >
      {surface === "canvas" && (
        <NodeResizer
          minWidth={minimum.w}
          minHeight={minimum.h}
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
            onClick={remove}
          >
            ✕
          </button>
        </span>
      </header>

      {/* React Flow claims pointer drags and wheel gestures anywhere on a node unless a subtree
          opts out: without `nodrag nowheel`, dragging a gain slider drags the node and scrolling
          a digit zooms the canvas instead of tuning. The header keeps both, so the node is
          dragged by its title bar — the patch-editor convention.
          The opt-out is only claimed by the *active* face: over every other one the wheel and the
          drag belong to the camera, so the patch stays navigable from wherever the pointer is. */}
      <div
        className={`flex min-h-0 flex-1 flex-col overflow-hidden ${
          active ? "nodrag nopan nowheel" : ""
        }`}
      >
        <Active value={active}>{children}</Active>
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

/** The face's own ✕. The engine call goes first (`closeEngineObjects`); only once it has landed
 * does the node leave the patch. */
function useRemoveNode(node: PatchNode): () => void {
  const station = useStationContext();
  const drop = useMutation({
    mutationFn: () => closeEngineObjects(station, [node.id]),
    onSuccess: () =>
      station.edit((snapshot) => {
        const graph = removeNode(snapshot.graph, node.id);
        return { ...snapshot, graph, rack: pruneRack(snapshot.rack ?? {}, graph) };
      }),
    onError: (error: Error) => pushToast(error.message),
  });

  return () => drop.mutate();
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
      // carries (DESIGN.md §2). A port that refuses everything carries the server's reason for it
      // — the operator finds out by pointing at it, not by dragging a wire at it.
      title={port.note == null ? `${port.name} (${port.port_type})` : `${port.name} — ${port.note}`}
      className={`!size-2.5 ${PORT_PAINT[port.port_type]} ${PORT_SHAPE[port.port_type]}`}
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
