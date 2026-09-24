import { create } from "zustand";

const KEY = "sdrmm.autoOffExplained";

export const useAutoOff = create<{ pending: (() => void) | null }>(() => ({ pending: null }));

function explained(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function leaveAuto(auto: boolean, proceed: () => void): void {
  if (!auto || explained()) {
    proceed();
    return;
  }
  useAutoOff.setState({ pending: proceed });
}

export function confirmLeave(neverAgain: boolean): void {
  if (neverAgain) {
    try {
      localStorage.setItem(KEY, "1");
    } catch {}
  }
  const proceed = useAutoOff.getState().pending;
  useAutoOff.setState({ pending: null });
  proceed?.();
}

export function cancelLeave(): void {
  useAutoOff.setState({ pending: null });
}
