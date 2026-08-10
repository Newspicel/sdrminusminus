// The error surface (DESIGN.md §5). Bottom-right, dismissible, and long-lived enough to read:
// a failure the operator missed is a silent failure.
import { useEffect } from "react";
import { type Toast, useToasts } from "../lib/toasts";
import { ICON_BTN } from "./controls";

/** Long enough to notice and read a server message, short enough that a stale one is not still
 * on screen when the next thing goes wrong. */
const LIFETIME_MS = 12_000;

export function Toasts() {
  const toasts = useToasts((state) => state.toasts);
  const dismiss = useToasts((state) => state.dismiss);
  return (
    <div
      // `aria-live` rather than `role="alert"` per toast: a burst of rejections should be read
      // in order, not interrupt itself.
      aria-live="polite"
      className="pointer-events-none fixed right-3 bottom-3 z-50 flex w-80 flex-col gap-2"
    >
      {toasts.map((toast) => (
        <ToastCard key={toast.id} toast={toast} onDismiss={() => dismiss(toast.id)} />
      ))}
    </div>
  );
}

function ToastCard({ toast, onDismiss }: { toast: Toast; onDismiss: () => void }) {
  useEffect(() => {
    const timer = window.setTimeout(onDismiss, LIFETIME_MS);
    return () => window.clearTimeout(timer);
    // `repeats` restarts the clock when the same failure happens again.
  }, [onDismiss, toast.repeats]);

  const error = toast.tone === "error";
  return (
    <div
      className={`pointer-events-auto flex items-start gap-2 rounded-md border bg-panel-3 p-2 pl-3 shadow-pop ${
        error ? "border-danger/60" : "border-line-strong"
      }`}
    >
      <span className={`legend pt-1 ${error ? "text-danger" : "text-ink-dim"}`}>
        {error ? "Rejected" : "Note"}
      </span>
      <span className="min-w-0 flex-1 font-mono text-xs break-words text-ink">
        {toast.message}
        {toast.repeats > 0 && <span className="text-ink-faint"> ×{toast.repeats + 1}</span>}
      </span>
      <button type="button" className={ICON_BTN} onClick={onDismiss} aria-label="Dismiss">
        ×
      </button>
    </div>
  );
}
