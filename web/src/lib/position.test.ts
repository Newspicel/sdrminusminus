import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { gridLocator, usePositionStore } from "./position";
import type { ClientCommand, PositionFix, ServerEvent } from "./types";
import type { SdrSocket } from "./ws";

const ignorePosition: PositionCallback = () => undefined;
const ignorePositionError: PositionErrorCallback = () => undefined;

function fix(latitude: number, over: Partial<PositionFix> = {}): PositionFix {
  return {
    latitude,
    longitude: 13.405,
    accuracy_m: 4,
    time: "2026-08-14T12:00:00Z",
    ...over,
  };
}

function event(position: PositionFix): ServerEvent {
  return {
    type: "PositionChanged",
    data: { node: "gps", fix: position },
  };
}

class PositionSocket {
  connected = true;
  sent: ClientCommand[] = [];
  listeners = new Set<(connected: boolean) => void>();

  send(command: ClientCommand): void {
    this.sent.push(command);
  }

  isConnected(): boolean {
    return this.connected;
  }

  addStatusListener(listener: (connected: boolean) => void): void {
    this.listeners.add(listener);
  }

  removeStatusListener(listener: (connected: boolean) => void): void {
    this.listeners.delete(listener);
  }

  emit(connected: boolean): void {
    this.connected = connected;
    for (const listener of this.listeners) {
      listener(connected);
    }
  }
}

function browserGlobals() {
  let success = ignorePosition;
  let failure = ignorePositionError;
  const clearWatch = vi.fn();
  const watchPosition = vi.fn((next: PositionCallback, error: PositionErrorCallback): number => {
    success = next;
    failure = error;
    return 7;
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout },
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { geolocation: { watchPosition, clearWatch } },
  });
  return {
    clearWatch,
    succeed(position: PositionFix): void {
      success({
        coords: {
          latitude: position.latitude,
          longitude: position.longitude,
          altitude: position.altitude_m ?? null,
          accuracy: position.accuracy_m ?? 0,
          altitudeAccuracy: null,
          heading: position.track_deg ?? null,
          speed: position.speed_mps ?? null,
          toJSON: () => ({}),
        },
        timestamp: Date.parse(position.time),
        toJSON: () => ({}),
      });
    },
    fail(message: string): void {
      failure({ code: 2, message, PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3 });
    },
  };
}

describe("gridLocator", () => {
  it("converts known station coordinates to six-character Maidenhead locators", () => {
    expect(gridLocator(52.52, 13.405)).toBe("JO62qm");
    expect(gridLocator(37.7749, -122.4194)).toBe("CM87ss");
  });

  it("keeps exact world edges inside the final field", () => {
    expect(gridLocator(90, 180)).toBe("RR99xx");
    expect(gridLocator(-90, -180)).toBe("AA00aa");
  });
});

describe("position history", () => {
  beforeEach(() => usePositionStore.getState().clear());

  it("replaces a duplicate location with its newest measurements", () => {
    usePositionStore.getState().observe(event(fix(52.52, { speed_mps: 1 })));
    usePositionStore.getState().observe(event(fix(52.52, { speed_mps: 2 })));
    const history = usePositionStore.getState().sources.gps?.history;
    expect(history).toHaveLength(1);
    expect(history?.[0]?.speed_mps).toBe(2);
  });

  it("caps each source at five thousand samples", () => {
    for (let latitude = 0; latitude <= 5_000; latitude += 1) {
      usePositionStore.getState().observe(event(fix(latitude)));
    }
    const history = usePositionStore.getState().sources.gps?.history;
    expect(history).toHaveLength(5_000);
    expect(history?.[0]?.latitude).toBe(1);
    expect(history?.at(-1)?.latitude).toBe(5_000);
  });
});

describe("device position watch", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
  });

  afterEach(() => vi.useRealTimers());

  it("replays fixes on reconnect, reports watch errors, and cleans up", async () => {
    const browser = browserGlobals();
    const socket = new PositionSocket();
    const { watchDevicePosition } = await import("./position");
    const cleanup = watchDevicePosition(socket as unknown as SdrSocket, ["gps"]);

    browser.succeed(fix(52.52));
    expect(socket.sent.at(-1)).toMatchObject({
      type: "PublishPosition",
      data: { node: "gps", fix: { latitude: 52.52 } },
    });
    const beforeReconnect = socket.sent.length;
    socket.emit(false);
    socket.emit(true);
    expect(socket.sent).toHaveLength(beforeReconnect + 1);

    browser.fail("receiver lost");
    expect(socket.sent.at(-1)).toMatchObject({
      type: "PublishPosition",
      data: { node: "gps", error: "receiver lost" },
    });
    cleanup();
    expect(browser.clearWatch).toHaveBeenCalledWith(7);
    expect(socket.listeners.size).toBe(0);
  });

  it("reports a missing geolocation provider", async () => {
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout },
    });
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: {} });
    const socket = new PositionSocket();
    const { watchDevicePosition } = await import("./position");
    const cleanup = watchDevicePosition(socket as unknown as SdrSocket, ["gps"]);
    expect(socket.sent.at(-1)).toMatchObject({
      type: "PublishPosition",
      data: { node: "gps", error: "this device has no geolocation provider" },
    });
    cleanup();
    expect(socket.listeners.size).toBe(0);
  });
});
