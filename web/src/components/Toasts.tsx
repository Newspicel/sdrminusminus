// The error surface (DESIGN.md §7). Bottom-right, dismissible, and long-lived enough to read:
// a failure the operator missed is a silent failure.
import { Toast } from "@base-ui/react/toast";
import { type ToastData, toastManager } from "../lib/toasts";
import { ICON_BTN } from "./controls";

/** Long enough to notice and read a server message, short enough that a stale one is not still
 * on screen when the next thing goes wrong. */
const LIFETIME_MS = 12_000;

/** Past this the stack is taller than it is readable. Over the limit the oldest cards are
 * marked `limited` and hidden, so a burst of failures never pushes the newest off the screen. */
const STACK_LIMIT = 4;

export function Toasts() {
  return (
    <Toast.Provider toastManager={toastManager} timeout={LIFETIME_MS} limit={STACK_LIMIT}>
      <Toast.Portal>
        {/* A flat column, not Base UI's stack: steady state is zero motion (DESIGN.md §11), and a
            card the operator has to hover to read is a card they will not read. */}
        <Toast.Viewport className="fixed right-3 bottom-3 z-50 flex w-80 max-w-[calc(100vw-1.5rem)] flex-col gap-2">
          <ToastList />
        </Toast.Viewport>
      </Toast.Portal>
    </Toast.Provider>
  );
}

function ToastList() {
  const { toasts } = Toast.useToastManager<ToastData>();
  return toasts.map((toast) => {
    const error = toast.type === "error";
    const repeats = toast.data?.repeats ?? 0;
    return (
      <Toast.Root
        key={toast.id}
        toast={toast}
        className={`rounded-md border bg-panel-3 shadow-pop data-limited:hidden ${
          error ? "border-danger/60" : "border-line-strong"
        }`}
      >
        <Toast.Content className="flex items-start gap-2 p-2 pl-3">
          <span className={`legend pt-1 ${error ? "text-danger" : "text-ink-dim"}`}>
            {error ? "Rejected" : "Note"}
          </span>
          <Toast.Title className="min-w-0 flex-1 font-mono text-xs break-words text-ink">
            {toast.title}
            {repeats > 0 && <span className="text-ink-faint"> ×{repeats + 1}</span>}
          </Toast.Title>
          <Toast.Close className={ICON_BTN} aria-label="Dismiss">
            ×
          </Toast.Close>
        </Toast.Content>
      </Toast.Root>
    );
  });
}
