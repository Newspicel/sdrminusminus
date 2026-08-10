// What every dock panel needs and none of it can receive as a prop: dockview serializes panel
// params into the layout, so a socket, a `DeviceSet` object or a setter cannot travel that way
// (PLAN §10). Panels read it from context; only ids ever reach the persisted layout.
import { createContext, type ReactNode, useContext } from "react";
import type { DeviceSet } from "../lib/types";
import type { SdrSocket } from "../lib/ws";

export interface Shell {
  socket: SdrSocket;
  connected: boolean;
  /** Every open device set — the decoder log labels its filter with them. */
  deviceSets: readonly DeviceSet[];
  /** The set the panels follow. A stored layout names no device set (engine ids are per-run),
   * so "which radio" is client state, chosen in the device bar. */
  active: DeviceSet | null;
  setActiveDs: (ds: number | null) => void;
  /** Selected channel *of the active set*: channel ids are allocated per set, so the selection
   * is cleared when the set changes rather than silently matching a different channel. */
  selectedChannel: number | null;
  setSelectedChannel: (channel: number | null) => void;
  /** Tuning, routed through the shell's optimistic patch pipelines so a click on the spectrum,
   * a keystroke and a channel row all take the same path. Clamped to the device's range. */
  tuneCenter: (hz: number) => void;
  tuneChannel: (channel: number, offsetHz: number) => void;
}

const ShellContext = createContext<Shell | null>(null);

export function ShellProvider({ value, children }: { value: Shell; children: ReactNode }) {
  return <ShellContext value={value}>{children}</ShellContext>;
}

export function useShell(): Shell {
  const shell = useContext(ShellContext);
  if (shell === null) {
    throw new Error("useShell outside ShellProvider");
  }
  return shell;
}
