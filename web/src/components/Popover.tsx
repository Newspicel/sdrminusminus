// The one non-modal overlay in the UI (DESIGN.md §9). Settings clusters and menus live here
// rather than in a bar row: they are consulted, not watched, and a row spent on them is a row
// the plot does not get.
//
// Dismissal is the whole contract — Esc, a click outside, and focus back on the trigger so a
// keyboard user is not dropped at the top of the document.
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
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
  const wrapRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) {
      return;
    }
    const onPointerDown = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        // Stopped here so a popover opened over the spectrum does not also reset its view.
        event.stopPropagation();
        setOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  }, [open]);

  return (
    <div ref={wrapRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        className={triggerClass}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen(!open)}
      >
        {label}
      </button>
      {open && (
        <div
          id={panelId}
          className={`absolute top-[calc(100%+4px)] z-30 ${align === "end" ? "right-0" : "left-0"} ${width} ${SURFACE} p-3`}
        >
          {children(() => {
            setOpen(false);
            triggerRef.current?.focus();
          })}
        </div>
      )}
    </div>
  );
}
