import { describe, expect, it } from "vitest";
import { step } from "./tabs";

describe("step", () => {
  it("wraps with the arrow keys", () => {
    expect(step(2, "ArrowRight", 3)).toBe(0);
    expect(step(0, "ArrowLeft", 3)).toBe(2);
  });

  it("jumps to the ends", () => {
    expect(step(1, "Home", 3)).toBe(0);
    expect(step(1, "End", 3)).toBe(2);
  });

  it("ignores other keys", () => {
    expect(step(1, "Enter", 3)).toBeNull();
  });
});
