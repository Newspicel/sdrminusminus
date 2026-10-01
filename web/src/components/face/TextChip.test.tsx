import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { chipText, TextChip } from "./TextChip";

const accept = () => true;

describe("chipText", () => {
  it("says none, or the placeholder, for an empty field", () => {
    expect(chipText("", false)).toBe("none");
    expect(chipText("", true, "any")).toBe("any");
  });

  it("never shows a secret", () => {
    expect(chipText("hunter2", true)).toBe("set");
  });

  it("shortens a long value", () => {
    expect(chipText("127.0.0.1:30005", false)).toBe("127.0.0.1:30005");
    const shown = chipText("https://hooks.example.com/services/abcdef", false);
    expect(shown).toHaveLength(22);
    expect(shown.endsWith("…")).toBe(true);
  });
});

describe("TextChip", () => {
  it("shows the value, named by its title", () => {
    const html = renderToStaticMarkup(
      <TextChip label="gpsd" title="GPSD address" value="127.0.0.1:2947" onCommit={accept} />,
    );
    expect(html).toContain(">127.0.0.1:2947<");
    expect(html).toContain('aria-label="GPSD address"');
    expect(html).toContain("font-medium text-ink");
  });

  it("goes quiet on the placeholder when empty", () => {
    const html = renderToStaticMarkup(
      <TextChip label="Contains" title="Text" value="" placeholder="any" onCommit={accept} />,
    );
    expect(html).toContain(">any<");
    expect(html).toContain("font-normal text-ink-dim");
  });

  it("shows a formatted value over the raw one", () => {
    const html = renderToStaticMarkup(
      <TextChip label="TG" title="Talkgroups" value="1, 2, 3" shown="3" onCommit={accept} />,
    );
    expect(html).toContain(">3<");
    expect(html).not.toContain("1, 2, 3");
  });
});
