import { Toast } from "@base-ui/react/toast";

export type Tone = "error" | "info";

export interface ToastData {
  /** How many times this message has repeated while its card was on screen. */
  repeats: number;
}

export const toastManager = Toast.createToastManager<ToastData>();

/** Live cards only: cleared on removal so a failure that comes back after the stack emptied
 * starts counting again rather than resuming an old tally. */
const repeats = new Map<string, number>();

export function pushToast(message: string, tone: Tone = "error"): void {
  // The message is the identity. Adding under an existing id updates the card in place and
  // restarts its timer, so one server refusing everything gets a counter, not a stack.
  const id = `${tone}:${message}`;
  const seen = (repeats.get(id) ?? -1) + 1;
  repeats.set(id, seen);
  toastManager.add({
    id,
    type: tone,
    title: message,
    data: { repeats: seen },
    onRemove: () => repeats.delete(id),
  });
}
