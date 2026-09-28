import { describe, expect, it } from "vitest";
import { hexUtf8 } from "./hex";

describe("hexUtf8", () => {
  it("writes each UTF-8 byte as two lowercase hex digits", () => {
    expect(hexUtf8("")).toBe("");
    expect(hexUtf8("sdrmm")).toBe("7364726d6d");
    expect(hexUtf8("ä€")).toBe("c3a4e282ac");
    expect(hexUtf8("\n")).toBe("0a");
  });
});
