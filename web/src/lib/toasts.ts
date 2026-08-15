import { Toast } from "@base-ui/react/toast";

export type Tone = "error" | "info";

export interface ToastData {
  repeats: number;
}

export const toastManager = Toast.createToastManager<ToastData>();

const repeats = new Map<string, number>();

export function pushToast(message: string, tone: Tone = "error"): void {
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
