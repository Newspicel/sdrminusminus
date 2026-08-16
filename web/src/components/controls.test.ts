import { describe, expect, it } from "vitest";
import { commitText, ICON_BTN, ICON_BTN_SM } from "./controls";

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

describe("commitText", () => {
  it("accepts an emptied field so an optional value can be cleared", () => {
    const seen: string[] = [];
    expect(
      commitText("  ", "TST", (value) => {
        seen.push(value);
        return true;
      }),
    ).toBe("");
    expect(seen).toEqual([""]);
  });

  it("keeps the current value when the field is committed unchanged", () => {
    const seen: string[] = [];
    expect(
      commitText(" TST ", "TST", (value) => {
        seen.push(value);
        return true;
      }),
    ).toBe("TST");
    expect(seen).toEqual([]);
  });

  it("restores the current value when the consumer refuses the edit", () => {
    expect(commitText("BAD", "TST", () => false)).toBe("TST");
  });
});
