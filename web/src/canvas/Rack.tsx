// The operate view (CANVAS §5): pinned faces on a snapping grid. Zero pan, zero zoom, no wires
// — operating wants alignment, density and muscle memory, and a second free canvas would just
// be the patch view with its wires hidden. If it feels cramped the answer is bigger cells, not
// a camera.
import { useCallback, useRef, useState } from "react";

import type { RackSlot } from "../lib/types";
import { useStationContext } from "./context";
import { placeSlot, RACK_COLS, RACK_ROWS } from "./graph";
import { FACES } from "./nodes";

/** What a pointer gesture on a slot is doing. Held in a ref: a drag must not re-render the rack
 * on its own bookkeeping. */
interface Gesture {
  node: string;
  mode: "move" | "resize";
  originX: number;
  originY: number;
  start: RackSlot;
}

export function Rack() {
  const station = useStationContext();
  const slots = station.rack.slots ?? [];
  const hostRef = useRef<HTMLDivElement>(null);
  const gesture = useRef<Gesture | null>(null);
  // The live cell during a drag, so the face follows the pointer before the write lands.
  const [preview, setPreview] = useState<RackSlot | null>(null);

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
      const next: RackSlot =
        active.mode === "move"
          ? { ...active.start, x: active.start.x + dx, y: active.start.y + dy }
          : { ...active.start, w: active.start.w + dx, h: active.start.h + dy };
      setPreview(next);
    },
    [cellSize],
  );

  const endGesture = useCallback(() => {
    const active = gesture.current;
    const next = preview;
    gesture.current = null;
    setPreview(null);
    if (active === null || next === null) {
      return;
    }
    // One write per gesture, not one per pointer move (CANVAS §4). A placement that would
    // overlap or leave the grid is refused by `placeSlot`, so the face simply stays put.
    station.edit((snapshot) => ({
      ...snapshot,
      rack: placeSlot(snapshot.rack ?? {}, active.node, {
        x: next.x,
        y: next.y,
        w: next.w,
        h: next.h,
      }),
    }));
  }, [preview, station]);

  const begin = (event: React.PointerEvent, slot: RackSlot, mode: Gesture["mode"]) => {
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    gesture.current = {
      node: slot.node,
      mode,
      originX: event.clientX,
      originY: event.clientY,
      start: slot,
    };
    setPreview(slot);
  };

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
        const shown = preview !== null && preview.node === slot.node ? preview : slot;
        const node = station.graph.nodes.find((candidate) => candidate.id === slot.node);
        if (node === undefined) {
          return null;
        }
        const Face = FACES[node.kind];
        return (
          <div
            key={slot.node}
            className="relative min-h-0 min-w-0"
            style={{
              gridColumn: `${shown.x + 1} / span ${shown.w}`,
              gridRow: `${shown.y + 1} / span ${shown.h}`,
            }}
          >
            <Face node={node} />
            {/* Whole-cell drag and resize; the grips sit over the shell's own chrome so a face's
                controls stay clickable. */}
            <span
              aria-hidden
              title="Move"
              className="absolute top-0 left-0 h-6.5 w-6 cursor-move"
              onPointerDown={(event) => begin(event, slot, "move")}
            />
            <span
              aria-hidden
              title="Resize"
              className="absolute right-0 bottom-0 size-3 cursor-se-resize border-r-2 border-b-2 border-line-strong"
              onPointerDown={(event) => begin(event, slot, "resize")}
            />
          </div>
        );
      })}
    </div>
  );
}
