// The operate view (CANVAS §5): pinned faces on a snapping grid. Zero pan, zero zoom, no wires
// — operating wants alignment, density and muscle memory, and a second free canvas would just
// be the patch view with its wires hidden.
//
// Three gestures, all in whole cells: the header drags a face (dropping it on another trades
// their places), an edge drags the boundary it shares with its neighbour so one grows as the
// other shrinks, and the corner resizes freely into whatever room there is. The arithmetic is
// `graph.ts`'s — this file is pointers and grips.
import { memo, useCallback, useRef, useState } from "react";

import type { PatchNode, RackLayout } from "../lib/types";
import { useStationContext } from "./context";
import { moveSlot, placeSlot, RACK_COLS, RACK_ROWS, type RackEdge, resizeSlot } from "./graph";
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
  const station = useStationContext();
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
      // A pointer moves far more often than it crosses a cell boundary, and every one of these
      // re-renders every face in the rack.
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
      // One write per gesture, not one per pointer move (CANVAS §4), and it is re-derived from
      // the stored rack rather than replayed from the preview: another client may have moved a
      // face while this drag was in flight, and their arrangement is the one to build on.
      station.edit((snapshot) => ({
        ...snapshot,
        rack: applyGesture({ ...active, base: snapshot.rack ?? {} }, dx, dy),
      }));
    },
    [cellSize, station],
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
      base: station.rack,
    };
  };

  const slots = (preview ?? station.rack).slots ?? [];
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
      className="grid min-h-0 flex-1 gap-px bg-line/40 p-px"
      style={{
        gridTemplateColumns: `repeat(${RACK_COLS}, minmax(0, 1fr))`,
        gridTemplateRows: `repeat(${RACK_ROWS}, minmax(0, 1fr))`,
      }}
      onPointerMove={onPointerMove}
      onPointerUp={endGesture}
      onPointerCancel={endGesture}
    >
      {slots.map((slot) => {
        const node = station.graph.nodes.find((candidate) => candidate.id === slot.node);
        if (node === undefined) {
          return null;
        }
        return (
          <div
            key={slot.node}
            className="relative min-h-0 min-w-0"
            style={{
              gridColumn: `${slot.x + 1} / span ${slot.w}`,
              gridRow: `${slot.y + 1} / span ${slot.h}`,
            }}
          >
            <RackFace node={node} />
            <Grips node={slot.node} onBegin={begin} />
          </div>
        );
      })}
    </div>
  );
}

/**
 * A face renders from its stored node and the station context, and neither changes while a
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
