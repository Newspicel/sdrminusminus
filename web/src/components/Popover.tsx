import type { ReactNode } from "react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { PopoverContent, PopoverTrigger, Popover as ShadcnPopover } from "@/components/ui/popover";
import { usePortalContainer } from "./PortalContainer";

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
  triggerClass?: string;
  align?: "start" | "end";
  width?: string;
  /** Off for content that owns its own edges — a tab strip that has to reach the popup's sides,
   * or panels that already pad themselves. */
  padded?: boolean;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const portalContainer = usePortalContainer();

  return (
    <ShadcnPopover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<Button variant="ghost" size="sm" className={triggerClass} />}>
        {label}
      </PopoverTrigger>
      <PopoverContent
        container={portalContainer}
        align={align}
        className={`${width} max-h-[var(--available-height)] max-w-[calc(100vw-1rem)] overflow-y-auto ${padded ? "p-3" : "p-0"}`}
      >
        {children(() => setOpen(false))}
      </PopoverContent>
    </ShadcnPopover>
  );
}
