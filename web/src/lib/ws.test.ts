import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SdrSocket } from "./ws";

const RECONNECT_CEILING = 30_000;

const sockets: FakeWebSocket[] = [];

class FakeWebSocket {
  static readonly OPEN = 1;
  readyState = 0;
  binaryType = "";
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor(readonly url: string) {
    sockets.push(this);
  }

  send(): void {}

  close(): void {
    this.readyState = 3;
  }

  accept(): void {
    this.readyState = 1;
    this.onopen?.();
  }

  drop(): void {
    this.readyState = 3;
    this.onclose?.();
  }
}

function fakeWindow() {
  const listeners = new Map<string, Set<() => void>>();
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: { protocol: "http:", host: "sdr.local" },
      setTimeout: (fn: () => void, ms: number) => globalThis.setTimeout(fn, ms),
      clearTimeout: (id: number) => globalThis.clearTimeout(id),
      addEventListener: (type: string, fn: () => void) => {
        const existing = listeners.get(type) ?? new Set<() => void>();
        existing.add(fn);
        listeners.set(type, existing);
      },
      removeEventListener: (type: string, fn: () => void) => {
        listeners.get(type)?.delete(fn);
      },
    },
  });
  return {
    fire: (type: string) => {
      for (const fn of listeners.get(type) ?? []) {
        fn();
      }
    },
    count: (type: string) => listeners.get(type)?.size ?? 0,
  };
}

function latest(): FakeWebSocket {
  const socket = sockets.at(-1);
  if (socket === undefined) {
    throw new Error("no socket was opened");
  }
  return socket;
}

describe("SdrSocket reconnect", () => {
  let online: ReturnType<typeof fakeWindow>;

  beforeEach(() => {
    sockets.length = 0;
    vi.useFakeTimers();
    Object.defineProperty(globalThis, "WebSocket", {
      configurable: true,
      value: FakeWebSocket,
    });
    vi.spyOn(Math, "random").mockReturnValue(1);
    online = fakeWindow();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("doubles the delay while the server refuses the connection", () => {
    const socket = new SdrSocket();
    socket.connect();
    expect(sockets).toHaveLength(1);

    latest().drop();
    vi.advanceTimersByTime(999);
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(2);

    latest().drop();
    vi.advanceTimersByTime(1_999);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(3);
  });

  it("keeps backing off when a connection opens and dies straight away", () => {
    const socket = new SdrSocket();
    socket.connect();

    latest().accept();
    latest().drop();
    vi.advanceTimersByTime(1_000);
    expect(sockets).toHaveLength(2);

    latest().accept();
    latest().drop();
    vi.advanceTimersByTime(1_999);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(3);
  });

  it("resets the delay only after a connection that lasted", () => {
    const socket = new SdrSocket();
    socket.connect();

    latest().drop();
    vi.advanceTimersByTime(1_000);
    expect(sockets).toHaveLength(2);

    latest().accept();
    vi.advanceTimersByTime(10_000);
    latest().drop();

    vi.advanceTimersByTime(999);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(3);
  });

  it("never waits longer than the ceiling", () => {
    const socket = new SdrSocket();
    socket.connect();

    for (let i = 0; i < 10; i++) {
      latest().drop();
      vi.advanceTimersByTime(RECONNECT_CEILING);
    }
    const seen = sockets.length;

    latest().drop();
    vi.advanceTimersByTime(RECONNECT_CEILING - 1);
    expect(sockets).toHaveLength(seen);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(seen + 1);
  });

  it("spreads the retry across the second half of the window", () => {
    vi.spyOn(Math, "random").mockReturnValue(0);
    const socket = new SdrSocket();
    socket.connect();

    latest().drop();
    vi.advanceTimersByTime(499);
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(2);
  });

  it("retries at once when the network comes back", () => {
    const socket = new SdrSocket();
    socket.connect();

    latest().drop();
    vi.advanceTimersByTime(1_000);
    latest().drop();
    const waiting = sockets.length;

    online.fire("online");
    expect(sockets).toHaveLength(waiting + 1);
  });

  it("goes back to the shortest delay after the network returns", () => {
    const socket = new SdrSocket();
    socket.connect();

    latest().drop();
    vi.advanceTimersByTime(1_000);
    latest().drop();
    vi.advanceTimersByTime(2_000);

    online.fire("online");
    latest().drop();
    vi.advanceTimersByTime(999);
    const waiting = sockets.length;
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(waiting + 1);
  });

  it("drops the network listener when the socket is closed", () => {
    const socket = new SdrSocket();
    socket.connect();
    expect(online.count("online")).toBe(1);

    socket.close();
    expect(online.count("online")).toBe(0);
  });

  it("stops reconnecting once closed", () => {
    const socket = new SdrSocket();
    socket.connect();
    const opened = sockets.length;

    socket.close();
    vi.advanceTimersByTime(60_000);
    expect(sockets).toHaveLength(opened);
  });
});
