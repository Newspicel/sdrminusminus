import { describe, expect, it } from "vitest";
import { type Chord, historyStep } from "./useHotkeys";

function chord(key: string, held: Partial<Chord> = {}): Chord {
  return { key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...held };
}

describe("historyStep", () => {
  it("takes both platforms' spelling of undo and redo", () => {
    expect(historyStep(chord("z", { metaKey: true }))).toBe("undo");
    expect(historyStep(chord("z", { ctrlKey: true }))).toBe("undo");
    expect(historyStep(chord("z", { metaKey: true, shiftKey: true }))).toBe("redo");
    expect(historyStep(chord("y", { ctrlKey: true }))).toBe("redo");
  });

  it("reads the shifted capital as the same key", () => {
    expect(historyStep(chord("Z", { ctrlKey: true, shiftKey: true }))).toBe("redo");
    expect(historyStep(chord("Y", { metaKey: true }))).toBe("redo");
  });

  it("leaves every other chord to the browser", () => {
    expect(historyStep(chord("z"))).toBeNull();
    expect(historyStep(chord("z", { altKey: true, metaKey: true }))).toBeNull();
    expect(historyStep(chord("s", { metaKey: true }))).toBeNull();
    expect(historyStep(chord("v", { ctrlKey: true }))).toBeNull();
  });
});
