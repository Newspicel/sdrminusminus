import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SymbolFrame } from "./frame";
import { ListenerRegistry } from "./listeners";
import { SymbolHub, type SymbolSocket } from "./symbols";
import type { ClientCommand } from "./types";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  const registry = new ListenerRegistry();
  const socket: SymbolSocket = {
    send: (command) => sent.push(command),
    on: (kind, listener) => registry.on(kind, listener),
  };
  return {
    socket,
    sent,
    started: (streamId: number, deviceSet: number, channel: number) =>
      registry.emit("event", {
        type: "SymbolStreamStarted",
        data: { stream_id: streamId, device_set: deviceSet, channel },
      }),
    push: (streamId: number, merDb = 18) =>
      registry.emit("symbols", {
        streamId,
        seq: 0,
        timestamp: 0n,
        plane: "level",
        symbolRate: 4800,
        evm: 0.12,
        merDb,
        margin: 2.4,
        freqErrorHz: -6,
        reference: Float32Array.from([1, 3, -1, -3]),
        symbols: Float32Array.from([1, -1, 3, -3]),
      }),
    reconnect: () => registry.emit("status", true),
  };
}

describe("SymbolHub", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("asks the server once however many faces watch one channel", () => {
    const fake = fakeSocket();
    const hub = new SymbolHub();
    hub.attach(fake.socket);

    const first: SymbolFrame[] = [];
    const second: SymbolFrame[] = [];
    hub.subscribe(1, 7, (block) => first.push(block));
    hub.subscribe(1, 7, (block) => second.push(block));
    expect(fake.sent).toEqual([{ type: "SubscribeSymbols", data: { device_set: 1, channel: 7 } }]);

    fake.started(300, 1, 7);
    fake.push(300);
    expect(first).toHaveLength(1);
    expect(second).toHaveLength(1);
    expect(first[0]?.merDb).toBe(18);
  });

  it("routes a block only to the channel that owns its stream", () => {
    const fake = fakeSocket();
    const hub = new SymbolHub();
    hub.attach(fake.socket);

    const mine: SymbolFrame[] = [];
    hub.subscribe(1, 7, (block) => mine.push(block));
    fake.started(300, 1, 7);
    fake.push(301);
    expect(mine).toHaveLength(0);

    fake.push(300);
    expect(mine).toHaveLength(1);
  });

  it("hands a late face the block that already arrived", () => {
    const fake = fakeSocket();
    const hub = new SymbolHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 7, () => {});
    fake.started(300, 1, 7);
    fake.push(300, 21);

    expect(hub.latest(1, 7)?.merDb).toBe(21);
    expect(hub.latest(1, 9)).toBeNull();
  });

  it("unsubscribes once the grace period passes with nobody watching", () => {
    const fake = fakeSocket();
    const hub = new SymbolHub();
    hub.attach(fake.socket);
    const drop = hub.subscribe(1, 7, () => {});
    drop();
    expect(fake.sent).toHaveLength(1);

    vi.advanceTimersByTime(10_000);
    expect(fake.sent[1]).toEqual({
      type: "UnsubscribeSymbols",
      data: { device_set: 1, channel: 7 },
    });
  });

  it("re-asks for everything it was watching after a reconnect", () => {
    const fake = fakeSocket();
    const hub = new SymbolHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 7, () => {});
    fake.sent.length = 0;

    fake.reconnect();
    expect(fake.sent).toEqual([{ type: "SubscribeSymbols", data: { device_set: 1, channel: 7 } }]);
  });
});
