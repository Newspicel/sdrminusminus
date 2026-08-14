import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SpectrumFrame } from "./frame";
import { SPECTRUM_HISTORY_ROWS, SpectrumHub, type SpectrumSocket } from "./spectrum";
import type { ClientCommand, ServerEvent } from "./types";

function fakeSocket() {
  const sent: ClientCommand[] = [];
  let frames: ((frame: SpectrumFrame) => void) | null = null;
  let status: ((connected: boolean) => void) | null = null;
  let events: ((event: ServerEvent) => void) | null = null;
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
    /** What the server sends when it answers a subscribe: the id it allocated for that lane. */
    started: (streamId: number, deviceSet: number, stream = 0) =>
      events?.({
        type: "StreamStarted",
        data: { stream_id: streamId, device_set: deviceSet, stream },
      }),
    stopped: (streamId: number) =>
      events?.({
        type: "StreamStopped",
        data: { stream_id: streamId, kind: "spectrum" },
      }),
    push: (streamId: number, bins: readonly number[] = [1, 2]) =>
      frames?.({
        streamId,
        seq: 0,
        timestamp: 0n,
        centerHz: 100e6,
        spanHz: 2e6,
        dbMin: -110,
        dbMax: -20,
        bins: Uint8Array.from(bins),
      }),
    reconnect: () => status?.(true),
    attached: () => frames !== null,
  };
}

const subscribes = (sent: ClientCommand[]) => sent.filter((c) => c.type === "SubscribeSpectrum");
const unsubscribes = (sent: ClientCommand[]) =>
  sent.filter((c) => c.type === "UnsubscribeSpectrum");

/** Past the release grace, whatever it is set to. */
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

  // A face is remounted by things that have nothing to do with its radio — the patch/rack switch
  // is the everyday one — and stopping the stream on the way through costs a restart the operator
  // sees as a stalled plot.
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
    // Neither a stop nor a second subscribe: the server is answering the first one still.
    expect(unsubscribes(fake.sent)).toHaveLength(0);
    expect(subscribes(fake.sent)).toHaveLength(1);

    fake.started(9, 1);
    fake.push(9);
    expect(seen).toEqual([9]);
  });

  // The id the server allocates is neither the device-set id nor the lane index, so a hub that
  // assumed either would route every frame to the wrong face — or to none.
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
    // An id nobody was told about, and the device-set ids themselves: neither is a stream id.
    fake.push(7);
    fake.push(1);
    expect(one).toEqual([41]);
    expect(two).toEqual([42]);
    expect(subscribes(fake.sent)).toHaveLength(2);
  });

  // Two scopes on two lanes of one multi-stream radio: independent subscriptions, and letting go
  // of one must leave the other running. Keying on the device set alone silenced both.
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
    // The surviving lane keeps delivering; only the released one goes quiet.
    fake.push(10);
    fake.push(11);
    expect(lane0).toEqual([10]);
    expect(lane2).toEqual([11, 11]);
    expect(hub.watched()).toEqual([{ deviceSet: 1, stream: 2 }]);
  });

  // Subscriptions are per-connection (): without this a dropped socket leaves every
  // scope face permanently blank.
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

  // Ids belong to the connection that issued them; a reconnect reissues them and an old one must
  // not still be routing frames to a face.
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

/** The rows of a history, one array per row, so an expectation reads as what the plot draws. */
function rowsOf(history: { rows: Uint8Array; count: number; bins: number }): number[][] {
  return Array.from({ length: history.count }, (_, row) => [
    ...history.rows.subarray(row * history.bins, (row + 1) * history.bins),
  ]);
}

// A plot's history lives in its own GL texture, so a face that remounts starts blank. These rows
// are what it opens on instead.
describe("SpectrumHub history", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("is empty for a lane nobody has watched", () => {
    const hub = new SpectrumHub();
    expect(hub.history(1, 0)).toEqual({ rows: new Uint8Array(0), count: 0, bins: 0 });
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
    // Frames still arrive while no face is watching, and they are the rows that make the switch
    // look like one continuous waterfall.
    fake.push(9, [5, 6]);

    expect(rowsOf(hub.history(1, 0))).toEqual([
      [1, 2],
      [3, 4],
      [5, 6],
    ]);
    expect(hub.latest(1, 0)?.centerHz).toBe(100e6);
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

  // A different bin count is a different x axis: the old rows cannot be drawn above the new ones.
  it("drops what it held when the bin count changes", () => {
    const fake = fakeSocket();
    const hub = new SpectrumHub();
    hub.attach(fake.socket);
    hub.subscribe(1, 0, () => {});
    fake.started(9, 1);
    fake.push(9, [1, 2]);
    fake.push(9, [7, 8, 9]);

    expect(rowsOf(hub.history(1, 0))).toEqual([[7, 8, 9]]);
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

  // The grace outlives a reconnect, so a lane inside one is a lane the new connection must be
  // told about — its face is on its way back and the history has to keep filling.
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
