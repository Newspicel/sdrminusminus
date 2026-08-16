import { describe, expect, it } from "vitest";
import { ListenerRegistry } from "./listeners";
import type { ServerEvent } from "./types";

const stopped: ServerEvent = {
  type: "StreamStopped",
  data: { stream_id: 1, kind: "spectrum" },
};

describe("ListenerRegistry", () => {
  it("delivers a payload to every listener of that kind", () => {
    const registry = new ListenerRegistry();
    const first: ServerEvent[] = [];
    const second: ServerEvent[] = [];
    registry.on("event", (event) => first.push(event));
    registry.on("event", (event) => second.push(event));

    registry.emit("event", stopped);

    expect(first).toEqual([stopped]);
    expect(second).toEqual([stopped]);
  });

  it("keeps kinds isolated from one another", () => {
    const registry = new ListenerRegistry();
    const seen: boolean[] = [];
    registry.on("status", (connected) => seen.push(connected));

    registry.emit("event", stopped);
    expect(seen).toEqual([]);

    registry.emit("status", true);
    expect(seen).toEqual([true]);
  });

  it("stops delivery once the returned disposer runs", () => {
    const registry = new ListenerRegistry();
    const seen: boolean[] = [];
    const off = registry.on("status", (connected) => seen.push(connected));

    registry.emit("status", true);
    off();
    registry.emit("status", false);

    expect(seen).toEqual([true]);
    expect(registry.count("status")).toBe(0);
  });

  it("drops only the listener its disposer belongs to", () => {
    const registry = new ListenerRegistry();
    const kept: boolean[] = [];
    const off = registry.on("status", () => {});
    registry.on("status", (connected) => kept.push(connected));
    expect(registry.count("status")).toBe(2);

    off();
    registry.emit("status", true);

    expect(registry.count("status")).toBe(1);
    expect(kept).toEqual([true]);
  });

  it("tolerates a listener unsubscribing while the batch is dispatching", () => {
    const registry = new ListenerRegistry();
    const seen: boolean[] = [];
    const off = registry.on("status", () => off());
    registry.on("status", (connected) => seen.push(connected));

    expect(() => registry.emit("status", true)).not.toThrow();
    expect(seen).toEqual([true]);
    expect(registry.count("status")).toBe(1);
  });

  it("ignores a disposer that runs more than once", () => {
    const registry = new ListenerRegistry();
    const off = registry.on("status", () => {});
    registry.on("status", () => {});

    off();
    off();

    expect(registry.count("status")).toBe(1);
  });
});
