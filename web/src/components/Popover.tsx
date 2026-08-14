import { Popover as Primitive } from "@base-ui/react/popover";
import type { ReactNode } from "react";
import { useState } from "react";
import { SURFACE } from "./controls";

export function Popover({
  label,
  triggerClass,
  align = "start",
  width = "w-80",
  padded = true,
  children,
}: {
  /** The trigger's content. Its accessible name comes from here, so it must read as an action. */
  label: ReactNode;
  triggerClass: string;
  align?: "start" | "end";
  width?: string;
  /** Off for content that owns its own edges — a tab strip that has to reach the popup's sides,
   * or panels that already pad themselves. */
  padded?: boolean;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);

  return (
    <Primitive.Root open={open} onOpenChange={setOpen}>
      <Primitive.Trigger className={triggerClass}>{label}</Primitive.Trigger>
      <Primitive.Portal>
        <Primitive.Positioner className="z-30" side="bottom" align={align} sideOffset={4}>
          {/* The popover is chrome, and chrome never widens the document or runs off the bottom
              of a phone: the width is the caller's, the ceiling is the viewport's. */}
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
