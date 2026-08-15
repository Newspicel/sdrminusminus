import { describe, expect, it, vi } from "vitest";
import type { TimeMachineAction, TimeMachineStatus } from "../lib/types";
import {
  heldSeconds,
  historyFill,
  timeMachineMutationOptions,
  timeMachinePhase,
} from "./timeMachine";

const held: TimeMachineStatus = {
  node: "history",
  stream: 0,
  history_seconds: 10,
  sample_rate: 2_048_000,
  center_hz: 100_000_000,
  held_samples: 10_240_000,
  capacity_samples: 20_480_000,
  overruns: 0,
};

const capturing: TimeMachineStatus = {
  ...held,
  capture: {
    file: "tm_1_20260816T000000Z",
    stream: 0,
    started_at: "2026-08-16T00:00:00Z",
    samples: 10_240_000,
    bytes: 81_920_000,
    overruns: 0,
  },
};

describe("timeMachinePhase", () => {
  it("separates an idle radio from an armed buffer and a running capture", () => {
    expect(timeMachinePhase({ status: "running" }, "history")).toEqual({ kind: "idle" });
    expect(timeMachinePhase({ status: "running", time_machine: held }, "history")).toEqual({
      kind: "armed",
      status: held,
    });
    expect(timeMachinePhase({ status: "running", time_machine: capturing }, "history")).toEqual({
      kind: "capturing",
      status: capturing,
    });
  });

  it("names the node that already holds the radio's history", () => {
    expect(timeMachinePhase({ status: "running", time_machine: held }, "other")).toEqual({
      kind: "busy",
      owner: "history",
    });
  });

  it("offers nothing while the radio is not running", () => {
    expect(timeMachinePhase({ status: "error", time_machine: held }, "history")).toEqual({
      kind: "unavailable",
    });
    expect(timeMachinePhase(null, "history")).toEqual({ kind: "unavailable" });
  });
});

describe("the held window", () => {
  it("reads as seconds and as a fraction of what was asked for", () => {
    expect(heldSeconds(held)).toBe(5);
    expect(historyFill(held)).toBe(0.5);
    expect(historyFill({ ...held, held_samples: 40_960_000 })).toBe(1);
    expect(historyFill({ ...held, capacity_samples: 0 })).toBe(0);
    expect(heldSeconds({ ...held, sample_rate: 0 })).toBe(0);
  });
});

describe("timeMachineMutationOptions", () => {
  it("sends each action to the radio holding the buffer", async () => {
    const seen: string[] = [];
    const request = vi.fn(async (_ds: number, action: TimeMachineAction) => {
      seen.push(action);
      return held;
    });
    const options = timeMachineMutationOptions(4, "history", 1, { history_seconds: 10 }, request);

    await options.mutationFn("arm");
    await options.mutationFn("capture");
    await options.mutationFn("stop");
    await options.mutationFn("disarm");

    expect(seen).toEqual(["arm", "capture", "stop", "disarm"]);
    expect(request).toHaveBeenLastCalledWith(4, "disarm", "history", 1, { history_seconds: 10 });
  });

  it("refuses an action with no radio wired in, and reports failures", async () => {
    const notify = vi.fn();
    const request = vi.fn(async () => held);
    const options = timeMachineMutationOptions(
      null,
      "history",
      0,
      { history_seconds: 10 },
      request,
      notify,
    );

    await expect(options.mutationFn("arm")).rejects.toThrow("Wire a running device's IQ");
    expect(request).not.toHaveBeenCalled();

    options.onError(new Error("that window does not fit in memory"));
    expect(notify).toHaveBeenCalledWith("that window does not fit in memory");
  });
});
