import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VideoFrame } from "./frame";
import type { ClientCommand, ServerEvent } from "./types";
import { VideoHub, type VideoSocket } from "./video";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  let frames: ((frame: VideoFrame) => void) | null = null;
  let status: ((connected: boolean) => void) | null = null;
  let events: ((event: ServerEvent) => void) | null = null;
  const socket: VideoSocket = {
    send: (command) => sent.push(command),
    addVideoListener: (listener) => {
      frames = listener;
    },
    removeVideoListener: () => {
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
        type: "VideoStreamStarted",
        data: { stream_id: streamId, device_set: deviceSet, channel },
      }),
    stopped: (streamId: number) =>
      events?.({
        type: "StreamStopped",
        data: { stream_id: streamId, kind: "video" },
      }),
    push: (streamId: number, width = 4, height = 2) =>
      frames?.({
        streamId,
        seq: 0,
        timestamp: 0n,
        width,
        height,
        format: "gray",
        pixels: new Uint8Array(width * height),
      }),
    reconnect: () => status?.(true),
    attached: () => frames !== null,
  };
}

const subscribes = (sent: ClientCommand[]) => sent.filter((c) => c.type === "SubscribeVideo");
const unsubscribes = (sent: ClientCommand[]) => sent.filter((c) => c.type === "UnsubscribeVideo");

const waitOutGrace = () => vi.advanceTimersByTime(60_000);

describe("VideoHub", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("subscribes once for many watchers of one channel and stops on the last one", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);

    const first = hub.subscribe(1, 2, () => {});
    const second = hub.subscribe(1, 2, () => {});
    expect(subscribes(fake.sent)).toHaveLength(1);

    first();
    waitOutGrace();
    expect(unsubscribes(fake.sent)).toHaveLength(0);
    second();
    waitOutGrace();
    expect(unsubscribes(fake.sent)).toHaveLength(1);
  });

  it("remounting inside the grace keeps the stream instead of replacing it", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);

    const stop = hub.subscribe(1, 2, () => {});
    stop();
    vi.advanceTimersByTime(100);
    hub.subscribe(1, 2, () => {});
    waitOutGrace();

    expect(subscribes(fake.sent)).toHaveLength(1);
    expect(unsubscribes(fake.sent)).toHaveLength(0);
  });

  it("routes frames by the id the server allocated, and to that channel only", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);

    const one: number[] = [];
    const two: number[] = [];
    hub.subscribe(1, 7, (frame) => one.push(frame.width));
    hub.subscribe(1, 8, (frame) => two.push(frame.width));
    fake.started(0x8000, 1, 7);
    fake.started(0x8001, 1, 8);

    fake.push(0x8000, 104);
    fake.push(0x8001, 164);
    fake.push(0x9999, 8);

    expect(one).toEqual([104]);
    expect(two).toEqual([164]);
  });

  it("keeps the last picture so a remounted face is not blank", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 2, () => {});
    fake.started(0x8000, 1, 2);

    expect(hub.latest(1, 2)).toBeNull();
    fake.push(0x8000, 104, 576);
    expect(hub.latest(1, 2)?.height).toBe(576);
    expect(hub.latest(9, 9)).toBeNull();
  });

  it("re-subscribes everything still watched after a reconnect", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 2, () => {});
    hub.subscribe(3, 4, () => {});
    expect(subscribes(fake.sent)).toHaveLength(2);

    fake.started(0x8000, 1, 2);
    fake.reconnect();
    const after: number[] = [];
    hub.subscribe(1, 2, (frame) => after.push(frame.width));
    fake.push(0x8000);
    expect(after).toEqual([]);

    expect(subscribes(fake.sent)).toHaveLength(4);
    fake.started(0x8000, 1, 2);
    fake.push(0x8000);
    expect(after).toHaveLength(1);
  });

  it("forgets an id the server says has stopped", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe(1, 2, (frame) => seen.push(frame.width));
    fake.started(0x8000, 1, 2);
    fake.push(0x8000);
    fake.stopped(0x8000);
    fake.push(0x8000);
    expect(seen).toHaveLength(1);
  });

  it("detaches from the socket it leaves", () => {
    const fake = fakeSocket();
    const hub = new VideoHub();
    hub.attach(fake.socket);
    expect(fake.attached()).toBe(true);
    hub.detach();
    expect(fake.attached()).toBe(false);
  });
});
