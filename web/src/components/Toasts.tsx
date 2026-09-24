import { Toast } from "@base-ui/react/toast";
import { Flag, X } from "lucide-react";
import { type ToastData, toastManager } from "../lib/toasts";
import { Button } from "./BaseControls";
import { BTN_SM, ICON_BTN } from "./controls";
import { Icon } from "./Icon";

const LIFETIME_MS = 12_000;

const STACK_LIMIT = 4;

export function Toasts({ onReport }: { onReport?: () => void }) {
  return (
    <Toast.Provider toastManager={toastManager} timeout={LIFETIME_MS} limit={STACK_LIMIT}>
      <Toast.Portal>
        <Toast.Viewport className="fixed right-3 bottom-3 z-50 flex w-80 max-w-[calc(100vw-1.5rem)] flex-col gap-2">
          <ToastList onReport={onReport} />
        </Toast.Viewport>
      </Toast.Portal>
    </Toast.Provider>
  );
}

function ToastList({ onReport }: { onReport?: () => void }) {
  const { toasts } = Toast.useToastManager<ToastData>();
  return toasts.map((toast) => {
    const error = toast.type === "error";
    const repeats = toast.data?.repeats ?? 0;
    const code = toast.data?.code;
    return (
      <Toast.Root
        key={toast.id}
        toast={toast}
        className={`rounded-md border bg-panel-3 shadow-pop data-limited:hidden ${
          error ? "border-danger/60" : "border-line-strong"
        }`}
      >
        <Toast.Content className="flex items-start gap-2 p-2 pl-3">
          <span
            className={`legend pt-1 ${error ? "text-danger" : "text-ink-dim"}`}
            title={code === undefined ? undefined : `Reported by the server as \`${code}\``}
          >
            {error ? (code ?? "Error") : "Note"}
          </span>
          <Toast.Title className="min-w-0 flex-1 font-mono text-xs break-words text-ink">
            {toast.title}
            {repeats > 0 && <span className="text-ink-faint"> ×{repeats + 1}</span>}
          </Toast.Title>
          {toast.actionProps !== undefined && <Toast.Action className={BTN_SM} />}
          {error && onReport !== undefined && (
            <Button
              type="button"
              className={ICON_BTN}
              aria-label="Report this problem"
              title="Report this problem, with the log around it"
              onClick={onReport}
            >
              <Icon glyph={Flag} />
            </Button>
          )}
          <Toast.Close className={ICON_BTN} aria-label="Dismiss">
            <Icon glyph={X} />
          </Toast.Close>
        </Toast.Content>
      </Toast.Root>
    );
  });
}
