import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ListenerRegistry } from "./listeners";
import {
  binsForView,
  resampleRows,
  SPECTRUM_HISTORY_ROWS,
  SpectrumHub,
  type SpectrumSocket,
} from "./spectrum";
import type { ClientCommand } from "./types";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  const registry = new ListenerRegistry();
  const socket: SpectrumSocket = {
    send: (command) => sent.push(command),
    isConnected: () => true,
    on: (kind, listener) => registry.on(kind, listener),
  };
  return {
    socket,
    sent,
    started: (streamId: number, deviceSet: number, stream = 0) =>
      registry.emit("event", {
        type: "StreamStarted",
        data: { stream_id: streamId, device_set: deviceSet, stream },
      }),
    stopped: (streamId: number) =>
      registry.emit("event", {
        type: "StreamStopped",
        data: { stream_id: streamId, kind: "spectrum" },
      }),
    push: (
      streamId: number,
      bins: readonly number[] = [1, 2],
      window: { dbMin: number; dbMax: number } = { dbMin: -110, dbMax: -20 },
    ) =>
      registry.emit("spectrum", {
        streamId,
        seq: 0,
        timestamp: 0n,
        centerHz: 100e6,
        spanHz: 2e6,
        dbMin: window.dbMin,
        dbMax: window.dbMax,
        bins: Uint8Array.from(bins),
      }),
    reconnect: () => registry.emit("status", true),
    attached: () => registry.count("spectrum") > 0,
  };
}

const subscribes = (sent: ClientCommand[]) => sent.filter((c) => c.type === "SubscribeSpectrum");
const unsubscribes = (sent: ClientCommand[]) =>
  sent.filter((c) => c.type === "UnsubscribeSpectrum");

const waitOutGrace = () => vi.advanceTimersByTime(60_000);

describe("SpectrumHub", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("subscribes once for many watchers of one lane and stops on the last one", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);

    const seen: number[] = [];
    const first = hub.subscribe(1, 0, () => seen.push(1));
    const second = hub.subscribe(1, 0, () => seen.push(2));
    expect(subscribes(fake.sent)).toHaveLength(1);

    fake.started(9, 1);
    fake.push(9);
    expect(seen).toEqual([1, 2]);

    first();
    expect(unsubscribes(fake.sent)).toHaveLength(0);
    fake.push(9);
    expect(seen).toEqual([1, 2, 2]);

    second();
    waitOutGrace();
    expect(unsubscribes(fake.sent)).toHaveLength(1);
    expect(hub.watched()).toEqual([]);
  });

  it("keeps the stream through a face that lets go and comes straight back", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);

    hub.subscribe(1, 0, () => {})();
    expect(hub.watched()).toEqual([{ deviceSet: 1, stream: 0 }]);
    expect(unsubscribes(fake.sent)).toHaveLength(0);

    const seen: number[] = [];
    hub.subscribe(1, 0, (frame) => seen.push(frame.streamId));
    waitOutGrace();
    expect(unsubscribes(fake.sent)).toHaveLength(0);
    expect(subscribes(fake.sent)).toHaveLength(1);

    fake.started(9, 1);
    fake.push(9);
    expect(seen).toEqual([9]);
  });

  it("routes frames by the id the server allocated, not by the device set", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    const one: number[] = [];
    const two: number[] = [];
    hub.subscribe(1, 0, (frame) => one.push(frame.streamId));
    hub.subscribe(2, 0, (frame) => two.push(frame.streamId));
    fake.started(41, 1);
    fake.started(42, 2);

    fake.push(41);
    fake.push(42);
    fake.push(7);
    fake.push(1);
    expect(one).toEqual([41]);
    expect(two).toEqual([42]);
    expect(subscribes(fake.sent)).toHaveLength(2);
  });

  it("keeps two lanes of one radio apart", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    const lane0: number[] = [];
    const lane2: number[] = [];
    const drop0 = hub.subscribe(1, 0, (frame) => lane0.push(frame.streamId));
    hub.subscribe(1, 2, (frame) => lane2.push(frame.streamId));
    expect(subscribes(fake.sent).map((c) => c.data.stream)).toEqual([0, 2]);

    fake.started(10, 1, 0);
    fake.started(11, 1, 2);
    fake.push(10);
    fake.push(11);
    expect(lane0).toEqual([10]);
    expect(lane2).toEqual([11]);

    drop0();
    waitOutGrace();
    expect(unsubscribes(fake.sent)).toEqual([
      { type: "UnsubscribeSpectrum", data: { device_set: 1, stream: 0 } },
    ]);
    fake.stopped(10);
    fake.push(10);
    fake.push(11);
    expect(lane0).toEqual([10]);
    expect(lane2).toEqual([11, 11]);
    expect(hub.watched()).toEqual([{ deviceSet: 1, stream: 2 }]);
  });

  it("re-subscribes every lane still watched when the socket comes back", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    hub.subscribe(1, 3, () => {});
    hub.subscribe(3, 0, () => {});
    fake.sent.length = 0;

    fake.reconnect();
    expect(
      subscribes(fake.sent)
        .map((c) => `${c.data.device_set}:${c.data.stream}`)
        .toSorted(),
    ).toEqual(["1:0", "1:3", "3:0"]);
  });

  it("forgets the ids of a connection that dropped", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    const seen: number[] = [];
    hub.subscribe(1, 0, (frame) => seen.push(frame.streamId));
    fake.started(5, 1);
    fake.push(5);

    fake.reconnect();
    fake.push(5);
    expect(seen, "the stale id routed a frame after the reconnect").toEqual([5]);

    fake.started(6, 1);
    fake.push(6);
    expect(seen).toEqual([5, 6]);
  });

  it("carries watchers onto a replacement socket and lets go of the old one", () => {
    const first = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(first.socket);
    hub.subscribe(2, 1, () => {});

    const second = fakeSocket();
    hub.attach(second.socket);
    expect(first.attached()).toBe(false);
    expect(subscribes(second.sent).map((c) => `${c.data.device_set}:${c.data.stream}`)).toEqual([
      "2:1",
    ]);

    hub.detach();
    expect(second.attached()).toBe(false);
  });
});

function rowsOf(history: { rows: Uint8Array; count: number; bins: number }): number[][] {
  return Array.from({ length: history.count }, (_, row) => [
    ...history.rows.subarray(row * history.bins, (row + 1) * history.bins),
  ]);
}

describe("spectrum resolution", () => {
  it("asks for more bins as the view narrows, up to the radio's FFT", () => {
    expect(binsForView(1)).toBe(1024);
    expect(binsForView(0.5)).toBe(2048);
    expect(binsForView(0.26)).toBe(4096);
    expect(binsForView(0.01)).toBe(4096);
    expect(binsForView(0)).toBe(1024);
  });

  it("keeps a narrow peak when rows lose resolution", () => {
    expect([...resampleRows(Uint8Array.from([0, 9, 0, 0]), 1, 4, 2)]).toEqual([9, 0]);
  });

  it("stretches rows that gain resolution", () => {
    expect([...resampleRows(Uint8Array.from([1, 2]), 1, 2, 4)]).toEqual([1, 1, 2, 2]);
  });
});

const zoomListener = () => {};

describe("SpectrumHub resolution", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("subscribes at the widest resolution any watcher wants and settles back", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    const dropWide = hub.subscribe(1, 0, () => {}, 4096);
    dropWide();

    const asked = subscribes(fake.sent).map((c) =>
      c.type === "SubscribeSpectrum" ? c.data.bins : 0,
    );
    expect(asked).toEqual([1024, 4096, 1024]);
  });

  it("re-subscribes when a watcher zooms in, once per change", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, zoomListener);
    hub.setBins(1, 0, zoomListener, 2048);
    hub.setBins(1, 0, zoomListener, 2048);

    const asked = subscribes(fake.sent).map((c) =>
      c.type === "SubscribeSpectrum" ? c.data.bins : 0,
    );
    expect(asked).toEqual([1024, 2048]);
  });

  it("ignores a zoom from a watcher it does not know", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    hub.setBins(1, 0, () => {}, 4096);

    expect(subscribes(fake.sent)).toHaveLength(1);
  });
});

describe("SpectrumHub history", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("is empty for a lane nobody has watched", () => {
    const hub = new SpectrumHub();
    expect(hub.history(1, 0)).toEqual({ rows: new Uint8Array(0), count: 0, bins: 0, meta: [] });
    expect(hub.latest(1, 0)).toBeNull();
  });

  it("keeps the rows of a lane whose face has gone, oldest first", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    const drop = hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    fake.push(9, [1, 2]);
    fake.push(9, [3, 4]);

    drop();
    fake.push(9, [5, 6]);

    expect(rowsOf(hub.history(1, 0))).toEqual([
      [1, 2],
      [3, 4],
      [5, 6],
    ]);
    expect(hub.latest(1, 0)?.centerHz).toBe(100e6);
  });

  it("keeps each row's own dB window, aligned with the rows", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    fake.push(9, [1, 2], { dbMin: -110, dbMax: -20 });
    fake.push(9, [3, 4], { dbMin: -100, dbMax: 0 });

    const history = hub.history(1, 0);
    expect(history.meta).toHaveLength(2);
    expect(history.meta[0]).toMatchObject({ dbMin: -110, dbMax: -20, spanHz: 2e6 });
    expect(history.meta[1]).toMatchObject({ dbMin: -100, dbMax: 0, centerHz: 100e6 });
  });

  it("keeps the metadata through a resolution change", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    fake.push(9, [1, 2]);
    fake.push(9, [7, 8, 9]);

    const history = hub.history(1, 0);
    expect(history.count).toBe(2);
    expect(history.meta).toHaveLength(2);
  });

  it("keeps the newest rows once the ring has wrapped", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    for (let row = 0; row < SPECTRUM_HISTORY_ROWS + 2; row++) {
      fake.push(9, [row & 0xff, 0]);
    }

    const history = hub.history(1, 0);
    expect(history.count).toBe(SPECTRUM_HISTORY_ROWS);
    const rows = rowsOf(history);
    expect(rows[0]).toEqual([2, 0]);
    expect(rows[rows.length - 1]).toEqual([(SPECTRUM_HISTORY_ROWS + 1) & 0xff, 0]);
  });

  it("rescales what it held when the bin count changes", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    fake.push(9, [1, 2]);
    fake.push(9, [7, 8, 9]);

    expect(rowsOf(hub.history(1, 0))).toEqual([
      [1, 2, 2],
      [7, 8, 9],
    ]);
  });

  it("forgets a lane nobody came back for", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    const drop = hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    fake.push(9, [1, 2]);

    drop();
    waitOutGrace();
    expect(hub.history(1, 0).count).toBe(0);
    expect(hub.latest(1, 0)).toBeNull();
    expect(unsubscribes(fake.sent)).toHaveLength(1);
  });

  it("re-subscribes a lane still inside its grace when the socket comes back", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {})();
    fake.sent.length = 0;

    fake.reconnect();
    expect(subscribes(fake.sent)).toHaveLength(1);
  });
});
