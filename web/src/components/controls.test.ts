import { describe, expect, it } from "vitest";
import { ICON_BTN, ICON_BTN_SM } from "./controls";

/** Every component in the app, as source. Vite's own glob rather than `node:fs`: the typecheck
 * runs without node types, and this is the toolchain's way to read a file at build time. */
const SOURCES: Record<string, string> = import.meta.glob("../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("icon buttons", () => {
  it("come in exactly two sizes", () => {
    expect(ICON_BTN).toContain("size-7");
    expect(ICON_BTN_SM).toContain("size-5");
  });

  /**
   * Two `size-*` utilities set the same properties, so the one Tailwind emits last wins whatever
   * order the call site writes them in — `${ICON_BTN} size-5` silently rendered at 28px, which is
   * what pushed a settings group's header row taller the moment it gained a remove button. The
   * size belongs to the constant; a call site that re-states one is the bug, not a preference.
   */
  it("are never resized at the call site", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([, text]) => /\$\{ICON_BTN(_SM)?\}[^`]*\bsize-\d/.test(text))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
