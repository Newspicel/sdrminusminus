// Every failure the operator must see, in one place (DESIGN.md §5). A banner row at the top of
// the shell moved every panel underneath it whenever the server refused something; a toast
// stack reports the same fact without the layout the operator did not ask for.
import { create } from "zustand";

export type Tone = "error" | "info";

export interface Toast {
  id: number;
  message: string;
  tone: Tone;
  /** Bumped when the same message repeats, so the card restarts its timer rather than stacking
   * duplicates of one server that is refusing everything. */
  repeats: number;
}

interface ToastStore {
  toasts: readonly Toast[];
  push: (message: string, tone?: Tone) => void;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToasts = create<ToastStore>((set) => ({
  toasts: [],
  push: (message, tone = "error") =>
    set((state) => {
      const existing = state.toasts.find((t) => t.message === message && t.tone === tone);
      if (existing !== undefined) {
        return {
          toasts: state.toasts.map((t) =>
            t.id === existing.id ? { ...t, repeats: t.repeats + 1 } : t,
          ),
        };
      }
      // Oldest first out: a burst of failures must not push the newest off the screen.
      return { toasts: [...state.toasts, { id: nextId++, message, tone, repeats: 0 }].slice(-4) };
    }),
  dismiss: (id) => set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),
}));

/** For non-React callers (mutation handlers, the socket) — same store, no hook. */
export function pushToast(message: string, tone: Tone = "error"): void {
  useToasts.getState().push(message, tone);
}
