import { describe, expect, it } from "vitest";
import { gridLocator } from "./position";

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
