import { Popover } from "@base-ui/react/popover";
import { useReactFlow } from "@xyflow/react";
import { useMemo } from "react";
import { SURFACE } from "../components/controls";
import { usePortalContainer } from "../components/PortalContainer";
import { NodePalette } from "./NodePalette";
import { useAddNode } from "./useAddNode";

export interface ScreenPoint {
  x: number;
  y: number;
}

export function CanvasPalette({ at, onClose }: { at: ScreenPoint; onClose: () => void }) {
  const add = useAddNode();
  const { screenToFlowPosition } = useReactFlow();
  const portalContainer = usePortalContainer();
  const anchor = useMemo(
    () => ({ getBoundingClientRect: () => new DOMRect(at.x, at.y, 0, 0) }),
    [at.x, at.y],
  );

  return (
    <Popover.Root
      open
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
    >
      <Popover.Portal container={portalContainer} className="contents">
        <Popover.Positioner
          className="z-40"
          anchor={anchor}
          side="bottom"
          align="start"
          collisionPadding={8}
        >
          <Popover.Popup className={`${SURFACE} w-[44rem] max-w-[calc(100vw-1rem)]`}>
            <NodePalette
              onAdd={(kind, channelType) => {
                add(kind, channelType, screenToFlowPosition(at));
                onClose();
              }}
            />
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
