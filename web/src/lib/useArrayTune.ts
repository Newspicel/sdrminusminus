import { tuneArray as sendArrayTune } from "./api";
import { useArrayStore } from "./arrays";
import { clearAction, failAction } from "./refusals";

export const TUNE_ACTION = "Tune";

export type TuneSettled = (error: unknown, idle: boolean) => void;

type Send = (node: string, hz: number) => Promise<unknown>;

interface Waiting {
  hz: number;
  settled: TuneSettled;
}

export function createTuneQueue(
  send: Send,
): (node: string, hz: number, settled: TuneSettled) => void {
  const waiting = new Map<string, Waiting>();
  const inFlight = new Set<string>();
  const run = (node: string): void => {
    const next = waiting.get(node);
    if (next === undefined) {
      inFlight.delete(node);
      return;
    }
    waiting.delete(node);
    inFlight.add(node);
    void send(node, next.hz)
      .then(
        () => next.settled(null, !waiting.has(node)),
        (error: unknown) => next.settled(error, !waiting.has(node)),
      )
      .finally(() => run(node));
  };
  return (node, hz, settled) => {
    waiting.set(node, { hz, settled });
    if (!inFlight.has(node)) {
      run(node);
    }
  };
}

const tuneQueue = createTuneQueue((node, hz) => sendArrayTune(node, { center_hz: hz }));

export function tuneSettled(node: string, hz: number): TuneSettled {
  return (error, idle) => {
    if (error === null) {
      clearAction(node, TUNE_ACTION);
    } else {
      failAction(node, TUNE_ACTION, error);
    }
    if (idle) {
      useArrayStore.getState().tuned(node, error === null ? hz : null);
    }
  };
}

function tuneArray(node: string, hz: number): void {
  useArrayStore.getState().retune(node, hz);
  tuneQueue(node, hz, tuneSettled(node, hz));
}

export function useArrayTune(): { tuneArray: (node: string, hz: number) => void } {
  return { tuneArray };
}
