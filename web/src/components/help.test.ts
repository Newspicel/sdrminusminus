import { describe, expect, it } from "vitest";
import { BINDINGS, type Binding } from "../canvas/useHotkeys";
import { DISCORD, DOCS, docsPage, groupBindings, host, isApple, keyCaps } from "./help";

describe("keyCaps", () => {
  it("splits keys into caps and separators", () => {
    expect(keyCaps("m / M", false)).toEqual([
      { text: "m", cap: true },
      { text: "/", cap: false },
      { text: "M", cap: true },
    ]);
    expect(keyCaps("1 – 9", false).map((part) => part.cap)).toEqual([true, false, true]);
  });

  it("shows only the modifier of the current platform", () => {
    expect(keyCaps("Ctrl / ⌘ Shift Z", true).map((part) => part.text)).toEqual(["⌘", "Shift", "Z"]);
    expect(keyCaps("Ctrl / ⌘ Z", false).map((part) => part.text)).toEqual(["Ctrl", "Z"]);
  });
});

describe("isApple", () => {
  it("recognises Apple platforms", () => {
    expect(isApple("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0)")).toBe(true);
    expect(isApple("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
  });
});

describe("host", () => {
  it("names where a link leads", () => {
    expect(host(DISCORD)).toBe("discord.gg");
    expect(host(docsPage("troubleshooting"))).toBe(new URL(DOCS).host);
    expect(host("")).toBe("");
  });
});

describe("groupBindings", () => {
  it("keeps groups and bindings in first-seen order", () => {
    const bindings: Binding[] = [
      { keys: "a", what: "A", group: "Tune" },
      { keys: "b", what: "B", group: "Edit" },
      { keys: "c", what: "C", group: "Tune" },
    ];
    expect(groupBindings(bindings)).toEqual([
      { group: "Tune", bindings: [bindings[0], bindings[2]] },
      { group: "Edit", bindings: [bindings[1]] },
    ]);
  });

  it("lists every binding exactly once", () => {
    const grouped = groupBindings(BINDINGS).flatMap((group) => group.bindings);
    expect(grouped).toHaveLength(BINDINGS.length);
    expect(new Set(grouped.map((binding) => binding.keys)).size).toBe(BINDINGS.length);
  });
});
