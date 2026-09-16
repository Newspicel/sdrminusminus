import { describe, expect, it } from "vitest";
import { siggenDeviceId, siggenKey } from "./signalGen";

describe("siggenKey", () => {
  it("makes a node name safe to use as a radio key", () => {
    expect(siggenKey("signal_gen:a1b2")).toBe("signal_gen-a1b2");
    expect(siggenDeviceId("signal_gen:a1b2")).toBe("siggen:signal_gen-a1b2");
    expect(siggenKey("plain")).toBe("plain");
  });

  it("gives each node its own generator", () => {
    expect(siggenDeviceId("signal_gen:a")).not.toBe(siggenDeviceId("signal_gen:b"));
  });
});
