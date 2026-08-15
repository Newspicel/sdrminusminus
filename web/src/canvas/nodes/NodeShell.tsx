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
        {/* The one place the node can be dragged from, so the one place that says so: the grab
          cursor is the affordance, and the library's default of painting it over the whole card
          promised a drag on every button inside the face. The buttons in here opt back out —
          they are pressed, not dragged. */}
        <header
          className={`flex h-6.5 shrink-0 items-center gap-2 border-b border-line bg-panel-2 pr-1 ${
            surface === "canvas" ? "cursor-grab active:cursor-grabbing" : ""
          }`}
        >
          <span aria-hidden className={`h-full w-1 ${CATEGORY_STRIP[category]}`} />
          <span className="legend truncate text-ink-dim">{node.label ?? title}</span>
          {subtitle !== undefined && (
            <span className="legend ml-auto truncate text-ink-faint">{subtitle}</span>
          )}
          <span className={`flex items-center gap-0.5 ${subtitle === undefined ? "ml-auto" : ""}`}>
            {actions}
            {/* On the rack or not is a state of the *patch*, and the rack is a view you may not be
              looking at — so it is carried by three things at once: a filled glyph against an
              empty one, the accent, and the pressed fill every other toggle in the kit uses.
              Colour alone would not survive a monochrome eye (). */}
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
              {pinned ? "▣" : "□"}
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

        {/* React Flow claims pointer drags and wheel gestures anywhere on a node unless a subtree
          opts out: without `nodrag nowheel`, dragging a gain slider drags the node and scrolling
          a digit zooms the canvas instead of tuning. The header keeps both, so the node is
          dragged by its title bar — the patch-editor convention.
          `nodrag` is unconditional: a face is not a drag handle whether or not it is selected,
          and a body that quietly moved the card was what put a grab cursor on every control in
          it. `nopan nowheel` stay conditional — over an inactive face the wheel and the drag
          belong to the camera, so the patch stays navigable from wherever the pointer is. */}
        <div
          className={`flex min-h-0 flex-1 flex-col overflow-hidden nodrag ${
            active ? "nopan nowheel" : ""
          }`}
        >
          <Active value={active}>{children}</Active>
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

/** The face's own ✕. The engine call goes first (`closeEngineObjects`); only once it has landed
 * does the node leave the patch. */
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

/** Position among the ports on the same side, so the two sides stack independently. */
function indexOnSide(ports: readonly PortSpec[], index: number): number {
  const side = ports[index]?.direction;
  return ports.slice(0, index).filter((port) => port.direction === side).length;
}

function PortHandle({
  port,
  label,
  offset,
}: {
  port: PortSpec;
  /** What the port is called on screen; the wire name stays `port.name` (`portLabel`). */
  label: string;
  offset: number;
}) {
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
      {/* : hue + marker shape + a *text* label, because with colour removed the graph
          must still be unambiguous.
          Outside the face rather than inset: a label over the body sits on whatever the instrument
          draws there, and a gutter wide enough for the longest port name would cost every face
          that much width. Level with its own handle, so the eye pairs the two without counting
          rows — and stacked over the wires rather than under them, on the canvas ground, so a
          connection running beneath stays readable. Inert, so a drag beginning here still belongs
          to the handle. */}
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

/**
 * The strip along the bottom of a face where what it *does* lives — open, forget, record, scan,
 * export, clear. One place per face, always the same place, so an action is never mistaken for a
 * setting and is never hunted for among them. Sits below the body whatever the body's height.
 */
export function FaceFooter({ children }: { children: ReactNode }) {
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-line p-2">
      {children}
    </div>
  );
}
