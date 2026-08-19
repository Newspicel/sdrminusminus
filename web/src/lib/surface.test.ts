import { describe, expect, it } from "vitest";
import type { RangeDopplerFrame } from "./frame";
import { ListenerRegistry } from "./listeners";
import { SurfaceHub, type SurfaceSocket } from "./surface";
import type { ClientCommand } from "./types";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  const registry = new ListenerRegistry();
  const socket: SurfaceSocket = {
    send: (command) => sent.push(command),
    isConnected: () => true,
    on: (kind, listener) => registry.on(kind, listener),
  };
  return {
    socket,
    sent,
    started: (streamId: number, node: string) =>
      registry.emit("event", {
        type: "SurfaceStreamStarted",
        data: { stream_id: streamId, device_set: 1, node },
      }),
    stopped: (streamId: number) =>
      registry.emit("event", {
        type: "StreamStopped",
        data: { stream_id: streamId, kind: "range_doppler" },
      }),
    push: (streamId: number, seq = 1) =>
      registry.emit("surface", {
        streamId,
        seq,
        timestamp: 0n,
        ranges: 4,
        dopplers: 3,
        rangeStepUs: 1,
        dopplerStepHz: 50,
        dbMin: -40,
        dbMax: 0,
        cells: new Uint8Array(12),
      } satisfies RangeDopplerFrame),
    reconnect: () => registry.emit("status", true),
  };
}

describe("SurfaceHub", () => {
  it("asks for a node once however many faces are watching", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const first = hub.subscribe("radar", () => {});
    const second = hub.subscribe("radar", () => {});
    expect(fake.sent.filter((c) => c.type === "SubscribeSurface")).toHaveLength(1);
    first();
    expect(fake.sent.filter((c) => c.type === "UnsubscribeSurface")).toHaveLength(0);
    second();
    expect(fake.sent.filter((c) => c.type === "UnsubscribeSurface")).toHaveLength(1);
  });

  it("routes a frame to the node its stream id was announced for", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe("radar", (frame) => seen.push(frame.seq));
    fake.push(9);
    expect(seen).toEqual([]);
    fake.started(9, "radar");
    fake.push(9, 4);
    expect(seen).toEqual([4]);
    expect(hub.latest("radar")?.seq).toBe(4);
  });

  it("drops a stream id once the server says the stream stopped", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe("radar", (frame) => seen.push(frame.seq));
    fake.started(9, "radar");
    fake.stopped(9);
    fake.push(9, 7);
    expect(seen).toEqual([]);
  });

  it("asks again for everything it was watching after a reconnect", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    hub.subscribe("radar", () => {});
    fake.sent.length = 0;
    fake.reconnect();
    expect(fake.sent).toEqual([{ type: "SubscribeSurface", data: { node: "radar" } }]);
    expect(hub.watched()).toEqual(["radar"]);
  });

  it("forgets nothing but sends nothing while detached", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.subscribe("radar", () => {});
    expect(fake.sent).toEqual([]);
    hub.attach(fake.socket);
    expect(fake.sent).toEqual([{ type: "SubscribeSurface", data: { node: "radar" } }]);
  });
});
