import { describe, expect, it } from "vitest";
import { beastState } from "./BeastOutputControls";

const status = {
  node: "beast",
  address: "127.0.0.1:30005",
  listening: true,
  clients: 2,
  frames: 10,
};

describe("beastState", () => {
  it("asks for a wire before anything else", () => {
    expect(beastState(null, false, false)).toBe("Wire ADS-B events in");
  });

  it("follows the server from closed to listening", () => {
    expect(beastState(null, true, false)).toBe("Server closed");
    expect(beastState(null, true, true)).toBe("Opening server");
    expect(beastState(status, true, true)).toBe("Listening");
  });

  it("leaves a failure to the fault block", () => {
    expect(beastState({ ...status, listening: false, error: "bind" }, true, true)).toBeUndefined();
  });
});
