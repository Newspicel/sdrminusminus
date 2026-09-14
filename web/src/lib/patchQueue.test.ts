import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceSettings } from "./types";
import { createPatchQueue } from "./useDevicePatch";

describe("createPatchQueue", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("sends the first change to a radio straight away", () => {
    const sent: DeviceSettings[] = [];
    const push = createPatchQueue((_ds, settings) => sent.push(settings));

    push(1, { center_hz: 100e6 });

    expect(sent).toEqual([{ center_hz: 100e6 }]);
  });

  it("carries only what the last of a burst asked for", () => {
    const sent: DeviceSettings[] = [];
    const push = createPatchQueue((_ds, settings) => sent.push(settings), 100);

    push(1, { center_hz: 100e6 });
    for (const hz of [100.1e6, 100.2e6, 100.3e6]) {
      vi.advanceTimersByTime(10);
      push(1, { center_hz: hz });
    }
    vi.advanceTimersByTime(100);

    expect(sent).toEqual([{ center_hz: 100e6 }, { center_hz: 100.3e6 }]);
  });

  it("loses no setting when a burst changes more than one", () => {
    const sent: DeviceSettings[] = [];
    const push = createPatchQueue((_ds, settings) => sent.push(settings), 100);

    push(1, { center_hz: 100e6 });
    push(1, { ppm: 5 });
    push(1, { center_hz: 101e6 });
    vi.advanceTimersByTime(100);

    expect(sent[1]).toEqual({ center_hz: 101e6, ppm: 5 });
  });

  it("holds each radio to its own pace", () => {
    const sent: [number, DeviceSettings][] = [];
    const push = createPatchQueue((ds, settings) => sent.push([ds, settings]), 100);

    push(1, { center_hz: 100e6 });
    push(2, { center_hz: 200e6 });

    expect(sent).toEqual([
      [1, { center_hz: 100e6 }],
      [2, { center_hz: 200e6 }],
    ]);
  });

  it("sends a change that arrives after the pause at once", () => {
    const sent: DeviceSettings[] = [];
    const push = createPatchQueue((_ds, settings) => sent.push(settings), 100);

    push(1, { center_hz: 100e6 });
    vi.advanceTimersByTime(500);
    push(1, { center_hz: 101e6 });

    expect(sent).toEqual([{ center_hz: 100e6 }, { center_hz: 101e6 }]);
  });
});
