import { Popover as Primitive } from "@base-ui/react/popover";
import type { ReactNode } from "react";
import { useState } from "react";
import { SURFACE } from "./controls";
import { usePortalContainer } from "./PortalContainer";

export function Popover({
  label,
  triggerClass,
  align = "start",
  width = "w-80",
  padded = true,
  children,
}: {
  label: ReactNode;
  triggerClass: string;
  align?: "start" | "end";
  width?: string;
  padded?: boolean;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const portalContainer = usePortalContainer();

  return (
    <Primitive.Root open={open} onOpenChange={setOpen}>
      <Primitive.Trigger className={triggerClass}>{label}</Primitive.Trigger>
      <Primitive.Portal container={portalContainer} className="contents">
        <Primitive.Positioner className="z-30" side="bottom" align={align} sideOffset={4}>
          <Primitive.Popup
            className={`${width} ${SURFACE} max-h-[var(--available-height)] max-w-[calc(100vw-1rem)] overflow-y-auto ${padded ? "p-3" : ""}`}
          >
            {children(() => setOpen(false))}
          </Primitive.Popup>
        </Primitive.Positioner>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
