import { afterEach, describe, expect, it } from "vitest";
import { arrayStatus } from "../test/fixtures";
import { shownCenterHz, useArrayStore } from "./arrays";
import { useRefusalStore } from "./refusals";
import { createTuneQueue, TUNE_ACTION, tuneSettled } from "./useArrayTune";

interface Pending {
  node: string;
  hz: number;
  finish: (error?: Error) => void;
}

function harness() {
  const sent: Pending[] = [];
  const outcomes: string[] = [];
  const tune = createTuneQueue(
    (node, hz) =>
      new Promise<void>((resolve, reject) => {
        sent.push({
          node,
          hz,
          finish: (error) => (error === undefined ? resolve() : reject(error)),
        });
      }),
  );
  const settled = (label: string) => (error: unknown, idle: boolean) =>
    outcomes.push(
      `${label} ${error === null ? "ok" : (error as Error).message}${idle ? " idle" : ""}`,
    );
  return { sent, outcomes, tune, settled };
}

async function flush(): Promise<void> {
  for (let round = 0; round < 4; round++) {
    await Promise.resolve();
  }
}

describe("createTuneQueue", () => {
  it("sends one tune per array at a time and only the newest of those that wait", async () => {
    const { sent, outcomes, tune, settled } = harness();
    tune("north", 100e6, settled("a"));
    tune("north", 101e6, settled("b"));
    tune("north", 102e6, settled("c"));
    tune("south", 50e6, settled("s"));
    expect(sent.map((request) => [request.node, request.hz])).toEqual([
      ["north", 100e6],
      ["south", 50e6],
    ]);
    sent[0]?.finish();
    await flush();
    expect(sent.map((request) => request.hz)).toEqual([100e6, 50e6, 102e6]);
    sent[2]?.finish(new Error("Held"));
    sent[1]?.finish();
    await flush();
    expect(outcomes).toEqual(["a ok", "c Held idle", "s ok idle"]);
    tune("north", 103e6, settled("d"));
    expect(sent.at(-1)?.hz).toBe(103e6);
  });
});

function tuneAlert(node: string): boolean {
  return (useRefusalStore.getState().byNode[node] ?? []).some(
    (refusal) => refusal.action === TUNE_ACTION,
  );
}

describe("tuneSettled", () => {
  afterEach(() => {
    useArrayStore.getState().reset();
    useRefusalStore.getState().reset();
  });

  it("holds the dial on the newest target until the last tune answers", () => {
    const store = useArrayStore.getState();
    store.seed([arrayStatus("north", { center_hz: 100e6 })]);
    store.retune("north", 101e6);
    tuneSettled("north", 101e6)(new Error("Held"), false);
    expect(shownCenterHz(useArrayStore.getState(), "north")).toBe(101e6);
    expect(tuneAlert("north")).toBe(true);
    tuneSettled("north", 102e6)(null, true);
    expect(shownCenterHz(useArrayStore.getState(), "north")).toBe(102e6);
    expect(tuneAlert("north")).toBe(false);
    store.retune("north", 103e6);
    tuneSettled("north", 103e6)(new Error("Held"), true);
    expect(shownCenterHz(useArrayStore.getState(), "north")).toBe(102e6);
  });
});
