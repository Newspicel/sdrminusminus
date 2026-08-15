import { type CSSProperties, memo, useCallback, useRef, useState } from "react";

import type { PatchNode, RackLayout } from "../lib/types";
import { useWorkspaceContext } from "./context";
import {
  type GraphContext,
  isResizable,
  moveSlot,
  naturalSize,
  placeSlot,
  RACK_COLS,
  RACK_ROWS,
  type RackEdge,
  resizeSlot,
} from "./graph";
import { FACES } from "./nodes";

/** A pointer gesture on a face. Held in a ref: a drag must not re-render the rack on its own
 * bookkeeping. */
interface Gesture {
  node: string;
  /** An edge drag takes the neighbour with it; the corner resizes this face alone. */
  mode: "move" | "corner" | RackEdge;
  originX: number;
  originY: number;
  /** The rack as it was when the gesture started — every frame is that layout plus one delta,
   * never the previous frame plus one, so a refused step cannot accumulate. */
  base: RackLayout;
}

export function Rack() {
  const workspace = useWorkspaceContext();
  const hostRef = useRef<HTMLDivElement>(null);
  const gesture = useRef<Gesture | null>(null);
  // The layout under the pointer, so the faces follow it before the write lands.
  const [preview, setPreview] = useState<RackLayout | null>(null);

  const cellSize = useCallback(() => {
    const host = hostRef.current;
    return host === null
      ? { w: 1, h: 1 }
      : { w: host.clientWidth / RACK_COLS, h: host.clientHeight / RACK_ROWS };
  }, []);

  const onPointerMove = useCallback(
    (event: React.PointerEvent) => {
      const active = gesture.current;
      if (active === null) {
        return;
      }
      const cell = cellSize();
      const dx = Math.round((event.clientX - active.originX) / cell.w);
      const dy = Math.round((event.clientY - active.originY) / cell.h);
      const next = applyGesture(active, dx, dy);
      setPreview((current) => (current !== null && sameRack(current, next) ? current : next));
    },
    [cellSize],
  );

  const endGesture = useCallback(
    (event: React.PointerEvent) => {
      const active = gesture.current;
      gesture.current = null;
      setPreview(null);
      if (active === null) {
        return;
      }
      const cell = cellSize();
      const dx = Math.round((event.clientX - active.originX) / cell.w);
      const dy = Math.round((event.clientY - active.originY) / cell.h);
      if (dx === 0 && dy === 0) {
        return;
      }
      workspace.edit((snapshot) => ({
        ...snapshot,
        rack: applyGesture({ ...active, base: snapshot.rack ?? {} }, dx, dy),
      }));
    },
    [cellSize, workspace],
  );

  const begin = (event: React.PointerEvent, node: string, mode: Gesture["mode"]) => {
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    gesture.current = {
      node,
      mode,
      originX: event.clientX,
      originY: event.clientY,
      base: workspace.rack,
    };
  };

  const slots = (preview ?? workspace.rack).slots ?? [];
  if (slots.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center bg-bg">
        <p className="text-sm text-ink-dim">
          Nothing pinned. Pin a node's face on the canvas to operate it here.
        </p>
      </div>
    );
  }

  return (
    <div
      ref={hostRef}
      // The patch's own ground: a bay is now usually wider than the face mounted in it, so what
      // shows between them is a wall rather than the hairline the separator colour was drawn for.
      className="grid min-h-0 flex-1 gap-px bg-bg p-px"
      style={{
        gridTemplateColumns: `repeat(${RACK_COLS}, minmax(0, 1fr))`,
        gridTemplateRows: `repeat(${RACK_ROWS}, minmax(0, 1fr))`,
      }}
      onPointerMove={onPointerMove}
      onPointerUp={endGesture}
      onPointerCancel={endGesture}
    >
      {slots.map((slot) => {
        const node = workspace.graph.nodes.find((candidate) => candidate.id === slot.node);
        if (node === undefined) {
          return null;
        }
        return (
          <div
            key={slot.node}
            // The same handle React Flow puts on a patch node, so a face is addressable by node
            // in either view.
            data-id={slot.node}
            className="flex min-h-0 min-w-0 items-center justify-center"
            style={{
              gridColumn: `${slot.x + 1} / span ${slot.w}`,
              gridRow: `${slot.y + 1} / span ${slot.h}`,
            }}
          >
            {/* A bay is how much of the wall the operator gave this instrument; the face inside
                it is still the size its kind is. Only a viewport grows into its bay — stretching
                a column of controls across one puts the same acre of dead space beside it that
                fixing the sizes removed from the patch. Never larger than the bay: a face given
                less room than it wants shrinks and scrolls rather than covering its neighbour. */}
            <div
              className="relative max-h-full max-w-full"
              style={faceSize(node, workspace.context)}
            >
              <RackFace node={node} />
              <Grips node={slot.node} onBegin={begin} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** The box the face gets inside its bay: the whole bay for an instrument that is worth more room,
 * and its own size for everything else (`isResizable`). */
function faceSize(node: PatchNode, context: GraphContext): CSSProperties {
  if (isResizable(node.kind)) {
    return { width: "100%", height: "100%" };
  }
  const size = naturalSize(node, context);
  return { width: size.w, height: size.h };
}

/**
 * A face renders from its stored node and the workspace context, and neither changes while a
 * pointer is down — so the drag preview, which re-renders the rack on every cell it crosses,
 * must not re-render the instruments with it. A scope's WebGL viewport and a map's tiles
 * repainting sixty times a second *is* the flicker this fixes.
 */
const RackFace = memo(function RackFace({ node }: { node: PatchNode }) {
  const Face = FACES[node.kind];
  return <Face node={node} />;
});

/** The grips sit over the shell's own chrome so a face's controls stay clickable: the header
 * drags (clear of the pin and remove buttons), the four edges drag their boundary, and the
 * bottom-right corner resizes into free space. */
function Grips({
  node,
  onBegin,
}: {
  node: string;
  onBegin: (event: React.PointerEvent, node: string, mode: Gesture["mode"]) => void;
}) {
  const edge = (mode: RackEdge, className: string, label: string) => (
    <span
      aria-hidden
      title={label}
      className={`absolute ${className}`}
      onPointerDown={(event) => onBegin(event, node, mode)}
    />
  );
  return (
    <>
      {edge("n", "inset-x-0 top-0 h-1.5 cursor-ns-resize", "Drag the boundary above")}
      {edge("s", "inset-x-0 bottom-0 h-1.5 cursor-ns-resize", "Drag the boundary below")}
      {edge("w", "inset-y-0 left-0 w-1.5 cursor-ew-resize", "Drag the boundary to the left")}
      {edge("e", "inset-y-0 right-0 w-1.5 cursor-ew-resize", "Drag the boundary to the right")}
      {/* Below the top edge grip, so both stay reachable on a 26px header. */}
      <span
        aria-hidden
        title="Move — drop on another face to trade places"
        className="absolute top-1.5 right-14 left-1.5 h-5 cursor-move"
        onPointerDown={(event) => onBegin(event, node, "move")}
      />
      <span
        aria-hidden
        title="Resize"
        className="absolute right-0 bottom-0 size-3.5 cursor-se-resize border-r-2 border-b-2 border-line-strong"
        onPointerDown={(event) => onBegin(event, node, "corner")}
      />
    </>
  );
}

/** The layout this gesture produces, `cells` from where it started. */
function applyGesture(gesture: Gesture, dx: number, dy: number): RackLayout {
  const slot = (gesture.base.slots ?? []).find((candidate) => candidate.node === gesture.node);
  if (slot === undefined) {
    return gesture.base;
  }
  switch (gesture.mode) {
    case "move":
      return moveSlot(gesture.base, gesture.node, { x: slot.x + dx, y: slot.y + dy });
    case "corner":
      return placeSlot(gesture.base, gesture.node, {
        x: slot.x,
        y: slot.y,
        w: slot.w + dx,
        h: slot.h + dy,
      });
    case "n":
      return resizeSlot(gesture.base, gesture.node, "n", dy);
    case "s":
      return resizeSlot(gesture.base, gesture.node, "s", dy);
    case "w":
      return resizeSlot(gesture.base, gesture.node, "w", dx);
    case "e":
      return resizeSlot(gesture.base, gesture.node, "e", dx);
  }
}

/** Structural equality, so a pointer move that stays inside its cell costs no render. */
function sameRack(a: RackLayout, b: RackLayout): boolean {
  return JSON.stringify(a.slots ?? []) === JSON.stringify(b.slots ?? []);
}
