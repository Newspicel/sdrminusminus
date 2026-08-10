// The one non-modal overlay in the UI (DESIGN.md §11). Settings clusters and menus live here
// rather than in a bar row: they are consulted, not watched, and a row spent on them is a row
// the plot does not get.
//
// Base UI owns dismissal (Esc, outside press, focus back on the trigger) and the nesting
// bookkeeping a portalled child popup needs — a `Select` opened inside a popover is outside its
// DOM subtree, so an outside-press rule written by hand would close the popover under the
// pointer.
import { Popover as Primitive } from "@base-ui/react/popover";
import type { ReactNode } from "react";
import { useState } from "react";
import { SURFACE } from "./controls";

export function Popover({
  label,
  triggerClass,
  align = "start",
  width = "w-80",
  children,
}: {
  /** The trigger's content. Its accessible name comes from here, so it must read as an action. */
  label: ReactNode;
  triggerClass: string;
  align?: "start" | "end";
  width?: string;
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
            className={`${width} ${SURFACE} max-h-[var(--available-height)] max-w-[calc(100vw-1rem)] overflow-y-auto p-3`}
          >
            {children(() => setOpen(false))}
          </Primitive.Popup>
        </Primitive.Positioner>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
