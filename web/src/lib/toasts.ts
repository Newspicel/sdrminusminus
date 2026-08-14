import { toast } from "sonner";

export type Tone = "error" | "info";

/** Live cards only: cleared on removal so a failure that comes back after the stack emptied
 * starts counting again rather than resuming an old tally. */
const repeats = new Map<string, number>();

export function pushToast(message: string, tone: Tone = "error"): void {
  // The message is the identity. Adding under an existing id updates the card in place and
  // restarts its timer, so one server refusing everything gets a counter, not a stack.
  const id = `${tone}:${message}`;
  const seen = (repeats.get(id) ?? -1) + 1;
  repeats.set(id, seen);
  const options = {
    id,
    description: seen > 0 ? `Repeated ${seen + 1} times` : undefined,
    onDismiss: () => repeats.delete(id),
    onAutoClose: () => repeats.delete(id),
  };
  if (tone === "error") {
    toast.error(message, options);
  } else {
    toast.info(message, options);
  }
}
