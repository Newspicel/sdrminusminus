import { Tooltip } from "@base-ui/react/tooltip";
import type { ReactElement, ReactNode } from "react";
import { SURFACE } from "./controls";
import { usePortalContainer } from "./PortalContainer";

const TIP_DELAY_MS = 250;

export function Tip({
  text,
  render,
  children,
}: {
  text: string;
  render: ReactElement;
  children?: ReactNode;
}) {
  const portalContainer = usePortalContainer();
  return (
    <Tooltip.Root>
      <Tooltip.Trigger render={render} delay={TIP_DELAY_MS}>
        {children}
      </Tooltip.Trigger>
      <Tooltip.Portal container={portalContainer} className="contents">
        <Tooltip.Positioner
          className="z-30 nodrag nopan"
          side="top"
          sideOffset={6}
          collisionPadding={8}
        >
          <Tooltip.Popup className={`${SURFACE} max-w-72 px-2 py-1 text-xs text-balance`}>
            {text}
          </Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
