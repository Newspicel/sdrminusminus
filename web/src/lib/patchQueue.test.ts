import { describe, expect, it } from "vitest";
import { ApiRequestError } from "./api";
import type { DeviceSettings } from "./types";
import { createPatchQueue, refusedByRadio } from "./useDevicePatch";

function recorder(): {
  sent: [number, DeviceSettings][];
  send: (ds: number, settings: DeviceSettings) => Promise<void>;
  settle: () => Promise<void>;
} {
  const sent: [number, DeviceSettings][] = [];
  const waiting: (() => void)[] = [];
  return {
    sent,
    send: (ds, settings) => {
      sent.push([ds, settings]);
      return new Promise<void>((resolve) => waiting.push(resolve));
    },
    settle: async () => {
      waiting.shift()?.();
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

describe("createPatchQueue", () => {
  it("sends the first change to a radio straight away", () => {
    const { sent, send } = recorder();
    const push = createPatchQueue(send);

    push(1, { center_hz: 100e6 });

    expect(sent).toEqual([[1, { center_hz: 100e6 }]]);
  });

  it("holds a burst behind the change already on its way", () => {
    const { sent, send } = recorder();
    const push = createPatchQueue(send);

    push(1, { center_hz: 100e6 });
    for (const hz of [100.1e6, 100.2e6, 100.3e6]) {
      push(1, { center_hz: hz });
    }

    expect(sent).toHaveLength(1);
  });

  it("carries only what the last of a held burst asked for", async () => {
    const { sent, send, settle } = recorder();
    const push = createPatchQueue(send);

    push(1, { center_hz: 100e6 });
    push(1, { center_hz: 100.1e6 });
    push(1, { center_hz: 100.3e6 });
    await settle();

    expect(sent).toEqual([
      [1, { center_hz: 100e6 }],
      [1, { center_hz: 100.3e6 }],
    ]);
  });

  it("loses no setting when a held burst changes more than one", async () => {
    const { sent, send, settle } = recorder();
    const push = createPatchQueue(send);

    push(1, { center_hz: 100e6 });
    push(1, { ppm: 5 });
    push(1, { center_hz: 101e6 });
    await settle();

    expect(sent[1]).toEqual([1, { center_hz: 101e6, ppm: 5 }]);
  });

  it("sends a change that arrives with nothing on its way at once", async () => {
    const { sent, send, settle } = recorder();
    const push = createPatchQueue(send);

    push(1, { center_hz: 100e6 });
    await settle();
    push(1, { center_hz: 101e6 });

    expect(sent).toHaveLength(2);
  });

  it("holds each radio to its own pace", () => {
    const { sent, send } = recorder();
    const push = createPatchQueue(send);

    push(1, { center_hz: 100e6 });
    push(2, { center_hz: 200e6 });

    expect(sent).toEqual([
      [1, { center_hz: 100e6 }],
      [2, { center_hz: 200e6 }],
    ]);
  });

  it("keeps sending after a change a radio refused", async () => {
    const sent: DeviceSettings[] = [];
    const push = createPatchQueue((_ds, settings) => {
      sent.push(settings);
      return Promise.reject(new Error("refused"));
    });

    push(1, { center_hz: 100e6 });
    await Promise.resolve();
    await Promise.resolve();
    push(1, { center_hz: 101e6 });

    expect(sent).toHaveLength(2);
  });
});

describe("refusedByRadio", () => {
  it("leaves a radio's refusal to its node and toasts the rest", () => {
    expect(refusedByRadio(new ApiRequestError("device I/O error", 500, "engine", undefined))).toBe(
      true,
    );
    expect(refusedByRadio(new ApiRequestError("locked", 400, "request", undefined))).toBe(false);
    expect(refusedByRadio(new Error("network"))).toBe(false);
  });
});
