import { Toast } from "@base-ui/react/toast";
import { describeError, recordEvent } from "./diagnostics";
import type { ErrorCode } from "./types";

export type Tone = "error" | "info";

export interface ToastData {
  repeats: number;
  code?: ErrorCode;
  detail?: string;
}

export const toastManager = Toast.createToastManager<ToastData>();

const repeats = new Map<string, number>();

export interface ToastDetail {
  code?: ErrorCode;
  detail?: string;
}

export function pushToast(message: string, tone: Tone = "error", extra: ToastDetail = {}): void {
  const id = `${tone}:${message}`;
  const seen = (repeats.get(id) ?? -1) + 1;
  repeats.set(id, seen);
  recordEvent(tone === "error" ? "error" : "info", "toast", toastRecord(message, extra));
  toastManager.add({
    id,
    type: tone,
    title: message,
    data: { repeats: seen, code: extra.code, detail: extra.detail },
    onRemove: () => repeats.delete(id),
  });
}

export interface ToastAction {
  label: string;
  run: () => void;
}

export function pushNote(message: string, action: ToastAction): void {
  recordEvent("info", "toast", message);
  toastManager.add({
    id: `info:${message}`,
    type: "info",
    title: message,
    data: { repeats: 0 },
    actionProps: { children: action.label, onClick: action.run },
  });
}

function toastRecord(message: string, extra: ToastDetail): string {
  const code = extra.code === undefined ? "" : `[${extra.code}] `;
  const detail = extra.detail === undefined ? "" : `: ${extra.detail}`;
  return `${code}${message}${detail}`;
}

export function toastError(error: unknown, tone: Tone = "error"): void {
  const carried = error as { code?: ErrorCode; detail?: string } | null;
  pushToast(messageOf(error), tone, {
    code: carried?.code,
    detail: carried?.detail,
  });
}

function messageOf(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return describeError(error);
}
