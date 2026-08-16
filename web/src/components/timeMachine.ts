import { controlTimeMachine } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type {
  DeviceSet,
  TimeMachineAction,
  TimeMachineNode,
  TimeMachineStatus,
} from "../lib/types";

export const MIN_HISTORY_SECONDS = 1;
export const MAX_HISTORY_SECONDS = 120;
export const DEFAULT_HISTORY_SECONDS = 10;
export const HISTORY_BYTES_PER_SAMPLE = 8;

export type TimeMachinePhase =
  | { kind: "unavailable" }
  | { kind: "idle" }
  | { kind: "armed"; status: TimeMachineStatus }
  | { kind: "capturing"; status: TimeMachineStatus }
  | { kind: "busy"; owner: string };

export function timeMachinePhase(
  set: { status: string; time_machine?: TimeMachineStatus | null } | null,
  node: string,
): TimeMachinePhase {
  if (set === null || set.status !== "running") {
    return { kind: "unavailable" };
  }
  const held = set.time_machine;
  if (held == null) {
    return { kind: "idle" };
  }
  if (held.node !== node) {
    return { kind: "busy", owner: held.node };
  }
  return held.capture == null
    ? { kind: "armed", status: held }
    : { kind: "capturing", status: held };
}

export function historyFill(status: TimeMachineStatus): number {
  return status.capacity_samples === 0
    ? 0
    : Math.min(1, status.held_samples / status.capacity_samples);
}

export function heldSeconds(status: TimeMachineStatus): number {
  return status.sample_rate === 0 ? 0 : status.held_samples / status.sample_rate;
}

export function timeMachineMutationOptions(
  deviceSet: number | null,
  node: string,
  stream: number,
  settings: TimeMachineNode,
  request: typeof controlTimeMachine = controlTimeMachine,
  notify: (message: string) => void = pushToast,
) {
  return {
    mutationFn: (action: TimeMachineAction) => {
      if (deviceSet === null) {
        return Promise.reject(new Error("Wire a running device's IQ into this sink first."));
      }
      return request(deviceSet, action, node, stream, settings);
    },
    onError: (error: Error) => notify(error.message),
  };
}

export function timeMachineOf(set: DeviceSet | null): TimeMachineStatus | null {
  return set?.time_machine ?? null;
}
