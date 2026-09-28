import { afterEach, describe, expect, it, vi } from "vitest";
import fixtures from "../generated/frame-fixtures.json";
import type { RangeDopplerFrame } from "./frame";
import { ListenerRegistry } from "./listeners";
import { type SurfaceFrame, SurfaceHub, type SurfaceSocket } from "./surface";
import type { ClientCommand } from "./types";
import { SdrSocket } from "./ws";

function rangeDoppler(streamId: number, seq: number): SurfaceFrame {
  const frame: RangeDopplerFrame = {
    streamId,
    seq,
    timestamp: 0n,
    ranges: 4,
    dopplers: 3,
    rangeFirstM: 0,
    rangeStepM: 150,
    dopplerFirstHz: -75,
    dopplerStepHz: 50,
    carrierHz: 98_000_000,
    dbMin: -40,
    dbMax: 0,
    cells: new Uint8Array(12),
  };
  return { kind: "range_doppler", frame };
}

function fakeSocket(connected = true) {
  const sent: ClientCommand[] = [];
  const registry = new ListenerRegistry();
  const socket: SurfaceSocket = {
    send: (command) => sent.push(command),
    isConnected: () => connected,
    on: (kind, listener) => registry.on(kind, listener),
  };
  return {
    socket,
    sent,
    subscribes: () => sent.filter((command) => command.type === "SubscribeSurface"),
    unsubscribes: () => sent.filter((command) => command.type === "UnsubscribeSurface"),
    started: (streamId: number, node: string) =>
      registry.emit("event", {
        type: "SurfaceStreamStarted",
        data: { stream_id: streamId, node, kind: "range_doppler" },
      }),
    stopped: (streamId: number, kind: "range_doppler" | "fusion_grid" = "range_doppler") =>
      registry.emit("event", { type: "StreamStopped", data: { stream_id: streamId, kind } }),
    push: (streamId: number, seq = 1) => registry.emit("surface", rangeDoppler(streamId, seq)),
    reconnect: () => registry.emit("status", true),
  };
}

function seqOf(surface: SurfaceFrame): number {
  return surface.frame.seq;
}

describe("SurfaceHub", () => {
  it("asks for a node once however many faces are watching", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const first = hub.subscribe("radar", () => {});
    const second = hub.subscribe("radar", () => {});
    expect(fake.subscribes()).toHaveLength(1);
    first();
    expect(fake.unsubscribes()).toHaveLength(0);
    second();
    expect(fake.unsubscribes()).toHaveLength(1);
  });

  it("a second listener on the same node sends nothing, and dropping one of two sends nothing", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    hub.subscribe("radar", () => {});
    const second = hub.subscribe("radar", () => {});
    second();
    expect(fake.subscribes()).toHaveLength(1);
    expect(fake.unsubscribes()).toHaveLength(0);
  });

  it("routes a frame to the node its stream id was announced for", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe("radar", (frame) => seen.push(seqOf(frame)));
    fake.push(9);
    expect(seen).toEqual([]);
    fake.started(9, "radar");
    fake.push(9, 4);
    expect(seen).toEqual([4]);
    expect(hub.latest("radar")?.kind).toBe("range_doppler");
  });

  it("drops a stream id once the server says the stream stopped", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe("radar", (frame) => seen.push(seqOf(frame)));
    fake.started(9, "radar");
    fake.stopped(9);
    fake.push(9, 7);
    expect(seen).toEqual([]);
  });

  it("drops the stream id of every surface kind", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe("grid", (frame) => seen.push(seqOf(frame)));
    fake.started(3, "grid");
    fake.stopped(3, "fusion_grid");
    fake.push(3, 2);
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

  it("sends the fit with the subscription and again after a reconnect", () => {
    const fake = fakeSocket();
    const hub = new SurfaceHub();
    hub.attach(fake.socket);
    hub.subscribe("radar", () => {}, { cols: 256, rows: 128 });
    hub.subscribe("radar", () => {}, { cols: 64, rows: 64 });
    fake.reconnect();
    expect(fake.subscribes()).toEqual([
      { type: "SubscribeSurface", data: { node: "radar", fit: { cols: 256, rows: 128 } } },
      { type: "SubscribeSurface", data: { node: "radar", fit: { cols: 256, rows: 128 } } },
    ]);
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

class FakeWebSocket {
  static readonly OPEN = 1;
  static last: FakeWebSocket | null = null;
  readyState = 0;
  binaryType = "";
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string | ArrayBuffer }) => void) | null = null;

  constructor() {
    FakeWebSocket.last = this;
  }

  send(): void {}

  close(): void {
    this.readyState = 3;
  }
}

function fixture(name: string): ArrayBuffer {
  const found = (fixtures as [string, number[]][]).find(([kind]) => kind === name);
  if (found === undefined) {
    throw new Error(`no ${name} fixture`);
  }
  return new Uint8Array(found[1]).buffer;
}

describe("socket surface frames", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("wraps each frame kind with its discriminant", () => {
    vi.stubGlobal("WebSocket", FakeWebSocket);
    vi.stubGlobal("window", {
      location: { protocol: "http:", host: "sdr.local" },
      addEventListener: () => {},
      removeEventListener: () => {},
      setTimeout: () => 0,
      clearTimeout: () => {},
    });
    const socket = new SdrSocket();
    const kinds: string[] = [];
    socket.on("surface", (surface) => kinds.push(surface.kind));
    socket.connect();
    for (const name of ["surface", "spatial_spectrum", "visibility", "fusion_grid"]) {
      FakeWebSocket.last?.onmessage?.({ data: fixture(name) });
    }
    socket.close();
    expect(kinds).toEqual(["range_doppler", "spatial_spectrum", "visibility", "fusion_grid"]);
  });
});
