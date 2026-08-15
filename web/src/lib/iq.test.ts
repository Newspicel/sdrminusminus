import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { IqFrame } from "./frame";
import { IqHub, type IqSocket } from "./iq";
import type { ClientCommand, ServerEvent } from "./types";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  let frames: ((frame: IqFrame) => void) | null = null;
  let status: ((connected: boolean) => void) | null = null;
  let events: ((event: ServerEvent) => void) | null = null;
  const socket: IqSocket = {
    send: (command) => sent.push(command),
    addIqListener: (listener) => {
      frames = listener;
    },
    removeIqListener: () => {
      frames = null;
    },
    addStatusListener: (listener) => {
      status = listener;
    },
    removeStatusListener: () => {
      status = null;
    },
    addEventListener: (listener) => {
      events = listener;
    },
    removeEventListener: () => {
      events = null;
    },
  };
  return {
    socket,
    sent,
    started: (streamId: number, deviceSet: number, channel: number) =>
      events?.({
        type: "IqStreamStarted",
        data: { stream_id: streamId, device_set: deviceSet, channel },
      }),
    stopped: (streamId: number) =>
      events?.({
        type: "StreamStopped",
        data: { stream_id: streamId, kind: "iq" },
      }),
    push: (streamId: number, samples: readonly number[] = [1, 0]) =>
      frames?.({
        streamId,
        seq: 0,
        timestamp: 0n,
        sampleRate: 24_000,
        centerHz: 145.8e6,
        samples: Float32Array.from(samples),
      }),
    reconnect: () => status?.(true),
    attached: () => frames !== null,
  };
}

describe("IqHub", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("subscribes once however many faces watch one channel", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);

    const first: IqFrame[] = [];
    const second: IqFrame[] = [];
    const dropFirst = hub.subscribe(1, 7, (frame) => first.push(frame));
    hub.subscribe(1, 7, (frame) => second.push(frame));
    expect(fake.sent).toEqual([{ type: "SubscribeIq", data: { device_set: 1, channel: 7 } }]);

    fake.started(0x8000, 1, 7);
    fake.push(0x8000, [1, 2]);
    expect(first).toHaveLength(1);
    expect(second).toHaveLength(1);

    dropFirst();
    vi.advanceTimersByTime(10_000);
    expect(fake.sent).toHaveLength(1);
  });

  it("stops the tap a grace period after the last watcher lets go", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);
    const drop = hub.subscribe(1, 7, () => {});

    drop();
    expect(fake.sent).toHaveLength(1);
    vi.advanceTimersByTime(10_000);
    expect(fake.sent[1]).toEqual({
      type: "UnsubscribeIq",
      data: { device_set: 1, channel: 7 },
    });
    expect(hub.watched()).toEqual([]);
  });

  it("cancels a pending stop rather than subscribing twice", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 7, () => {})();

    hub.subscribe(1, 7, () => {});
    vi.advanceTimersByTime(10_000);
    expect(fake.sent).toEqual([{ type: "SubscribeIq", data: { device_set: 1, channel: 7 } }]);
  });

  it("routes frames by the id the server allocated, not the channel", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);
    const seven: IqFrame[] = [];
    const eight: IqFrame[] = [];
    hub.subscribe(1, 7, (frame) => seven.push(frame));
    hub.subscribe(1, 8, (frame) => eight.push(frame));
    fake.started(0x8000, 1, 7);
    fake.started(0x8001, 1, 8);

    fake.push(0x8001, [3, 4]);
    expect(seven).toHaveLength(0);
    expect(Array.from(eight[0]?.samples ?? [])).toEqual([3, 4]);
  });

  it("drops frames for an id that has been stopped", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);
    const seen: IqFrame[] = [];
    hub.subscribe(1, 7, (frame) => seen.push(frame));
    fake.started(0x8000, 1, 7);
    fake.stopped(0x8000);
    fake.push(0x8000);
    expect(seen).toHaveLength(0);
  });

  it("keeps only the newest burst", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 7, () => {});
    fake.started(0x8000, 1, 7);

    fake.push(0x8000, [1, 1]);
    fake.push(0x8000, [2, 2]);
    expect(Array.from(hub.latest(1, 7)?.samples ?? [])).toEqual([2, 2]);
    expect(hub.latest(1, 9)).toBeNull();
  });

  it("re-sends every watched tap after a reconnect", () => {
    const fake = fakeSocket();
    const hub = new IqHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 7, () => {});
    fake.started(0x8000, 1, 7);
    fake.sent.length = 0;

    fake.reconnect();
    expect(fake.sent).toEqual([{ type: "SubscribeIq", data: { device_set: 1, channel: 7 } }]);

    const seen: IqFrame[] = [];
    hub.subscribe(1, 7, (frame) => seen.push(frame));
    fake.push(0x8000);
    expect(seen).toHaveLength(0);
  });

  it("detaches from the socket it leaves", () => {
    const first = fakeSocket();
    const second = fakeSocket();
    const hub = new IqHub();
    hub.attach(first.socket);
    hub.attach(second.socket);
    expect(first.attached()).toBe(false);
    expect(second.attached()).toBe(true);

    hub.detach();
    expect(second.attached()).toBe(false);
  });
});
