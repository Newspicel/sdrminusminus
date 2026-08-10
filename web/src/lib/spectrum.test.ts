import { describe, expect, it } from "vitest";
import type { SpectrumFrame } from "./frame";
import { SpectrumHub, type SpectrumSocket } from "./spectrum";
import type { ClientCommand } from "./types";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  let frames: ((frame: SpectrumFrame) => void) | null = null;
  let status: ((connected: boolean) => void) | null = null;
  const socket: SpectrumSocket = {
    send: (command) => sent.push(command),
    isConnected: () => true,
    addSpectrumListener: (listener) => {
      frames = listener;
    },
    removeSpectrumListener: () => {
      frames = null;
    },
    addStatusListener: (listener) => {
      status = listener;
    },
    removeStatusListener: () => {
      status = null;
    },
  };
  return {
    socket,
    sent,
    push: (streamId: number) => frames?.({ streamId } as SpectrumFrame),
    reconnect: () => status?.(true),
    attached: () => frames !== null,
  };
}

const subscribes = (sent: ClientCommand[]) => sent.filter((c) => c.type === "SubscribeSpectrum");
const unsubscribes = (sent: ClientCommand[]) =>
  sent.filter((c) => c.type === "UnsubscribeSpectrum");

describe("SpectrumHub", () => {
  it("subscribes once for many watchers of one radio and stops on the last one", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);

    const seen: number[] = [];
    const first = hub.subscribe(1, () => seen.push(1));
    const second = hub.subscribe(1, () => seen.push(2));
    expect(subscribes(fake.sent)).toHaveLength(1);

    fake.push(1);
    expect(seen).toEqual([1, 2]);

    first();
    expect(unsubscribes(fake.sent)).toHaveLength(0);
    fake.push(1);
    expect(seen).toEqual([1, 2, 2]);

    second();
    expect(unsubscribes(fake.sent)).toHaveLength(1);
    expect(hub.watched()).toEqual([]);
  });

  it("routes frames by device set and ignores unwatched streams", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    const one: number[] = [];
    const two: number[] = [];
    hub.subscribe(1, (frame) => one.push(frame.streamId));
    hub.subscribe(2, (frame) => two.push(frame.streamId));

    fake.push(1);
    fake.push(2);
    fake.push(7);
    expect(one).toEqual([1]);
    expect(two).toEqual([2]);
    expect(subscribes(fake.sent)).toHaveLength(2);
  });

  // Subscriptions are per-connection (PLAN §5): without this a dropped socket leaves every
  // scope face permanently blank.
  it("re-subscribes everything still watched when the socket comes back", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, () => {});
    hub.subscribe(3, () => {});
    fake.sent.length = 0;

    fake.reconnect();
    expect(
      subscribes(fake.sent)
        .map((c) => c.data.device_set)
        .toSorted((a, b) => a - b),
    ).toEqual([1, 3]);
  });

  it("carries watchers onto a replacement socket and lets go of the old one", () => {
    const first = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(first.socket);
    hub.subscribe(2, () => {});

    const second = fakeSocket();
    hub.attach(second.socket);
    expect(first.attached()).toBe(false);
    expect(subscribes(second.sent).map((c) => c.data.device_set)).toEqual([2]);

    hub.detach();
    expect(second.attached()).toBe(false);
  });
});
