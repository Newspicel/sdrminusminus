import type { RemoteState, RemoteStatus } from "../lib/types";

const POLL_MS = 2000;
const POLLED: ReadonlySet<RemoteState> = new Set(["pairing", "connecting", "retrying"]);

export function remotePollMs(status: RemoteStatus | undefined): number | false {
  return status !== undefined && POLLED.has(status.state) ? POLL_MS : false;
}

export function appHost(origin: string): string {
  try {
    return new URL(origin).host;
  } catch {
    return origin;
  }
}

export type RemoteAction = "pair" | "cancel" | "disconnect" | "pair-again" | null;

export function remoteAction(status: RemoteStatus): RemoteAction {
  if (status.via_relay) {
    return null;
  }
  switch (status.state) {
    case "unpaired":
      return "pair";
    case "pairing":
      return "cancel";
    case "rejected":
      return "pair-again";
    default:
      return "disconnect";
  }
}

export function remoteLine(status: RemoteStatus): string {
  switch (status.state) {
    case "online":
      return "Online";
    case "connecting":
      return "Connecting";
    case "retrying":
      return status.error ? `Offline: ${status.error}` : "Offline";
    case "rejected":
      return status.error ? `Disconnected: ${status.error}` : "Disconnected";
    default:
      return "";
  }
}
