// The `?` overlay (DESIGN.md §8). Its only job is to make the keyboard layer discoverable, so
// it is one table read straight from the binding list the handler uses.
import { useEffect, useRef } from "react";
import { BINDINGS } from "../shell/useHotkeys";
import { BTN, SURFACE } from "./controls";

export function Shortcuts({ onClose }: { onClose: () => void }) {
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("keydown", onKeyDown, true);
    return () => document.removeEventListener("keydown", onKeyDown, true);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-bg/70 p-4"
      // The scrim is the dismiss target; the panel stops the click so a drag inside it does not
      // close the overlay on release.
      onPointerDown={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Keyboard shortcuts"
        className={`${SURFACE} w-full max-w-md p-4`}
        onPointerDown={(event) => event.stopPropagation()}
      >
        <div className="flex items-baseline justify-between gap-4">
          <h2 className="text-base font-medium text-ink">Keyboard</h2>
          <span className="legend">Ignored while a field has focus</span>
        </div>
        <dl className="mt-3 grid grid-cols-[8rem_1fr] gap-x-4 gap-y-1.5">
          {BINDINGS.map((binding) => (
            <div key={binding.keys} className="contents">
              <dt className="text-right font-mono text-xs text-accent">{binding.keys}</dt>
              <dd className="text-xs text-ink-dim">{binding.what}</dd>
            </div>
          ))}
        </dl>
        <div className="mt-4 flex justify-end">
          <button ref={closeRef} type="button" className={BTN} onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
