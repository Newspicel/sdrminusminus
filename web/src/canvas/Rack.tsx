import { type CSSProperties, memo, useCallback, useRef, useState } from "react";

import type { PatchNode, RackLayout } from "../lib/types";
import { useWorkspaceContext } from "./context";
import {
  moveSlot,
  NODE_SIZE,
  placeSlot,
  RACK_COLS,
  RACK_ROWS,
  type RackEdge,
  resizeSlot,
} from "./graph";
import { FACES } from "./nodes";

interface Gesture {
  node: string;
  mode: "move" | "corner" | RackEdge;
  originX: number;
  originY: number;
  base: RackLayout;
}

export function Rack() {
  const workspace = useWorkspaceContext();
  const hostRef = useRef<HTMLDivElement>(null);
  const gesture = useRef<Gesture | null>(null);
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
            data-id={slot.node}
            className="flex min-h-0 min-w-0 items-center justify-center"
            style={{
              gridColumn: `${slot.x + 1} / span ${slot.w}`,
              gridRow: `${slot.y + 1} / span ${slot.h}`,
            }}
          >
            <div className="relative max-h-full max-w-full" style={faceSize(node)}>
              <RackFace node={node} />
              <Grips node={slot.node} onBegin={begin} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

function faceSize(node: PatchNode): CSSProperties {
  const size = NODE_SIZE[node.kind];
  return { width: size.h === undefined ? size.w : "100%", height: "100%" };
}

const RackFace = memo(function RackFace({ node }: { node: PatchNode }) {
  const Face = FACES[node.kind];
  return <Face node={node} />;
});

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

function sameRack(a: RackLayout, b: RackLayout): boolean {
  return JSON.stringify(a.slots ?? []) === JSON.stringify(b.slots ?? []);
}
