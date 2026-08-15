import { describe, expect, it } from "vitest";
import { ICON_BTN, ICON_BTN_SM } from "./controls";

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

  it("are never resized at the call site", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([, text]) => /\$\{ICON_BTN(_SM)?\}[^`]*\bsize-\d/.test(text))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});
