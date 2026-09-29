import { describe, expect, it } from "vitest";
import { cleanLinks } from "./links";

describe("cleanLinks", () => {
  it("drops the extension from page links", () => {
    expect(cleanLinks('<a href="../hardware.html">')).toBe('<a href="../hardware">');
    expect(cleanLinks('<a href="server/deployment.html#docker">')).toBe(
      '<a href="server/deployment#docker">',
    );
    expect(cleanLinks('<a href="/download.html?from=docs">')).toBe(
      '<a href="/download?from=docs">',
    );
  });

  it("points index pages at their folder", () => {
    expect(cleanLinks('<a href="index.html">')).toBe('<a href="./">');
    expect(cleanLinks('<a href="../index.html">')).toBe('<a href="../">');
  });

  it("cleans search results", () => {
    expect(cleanLinks('{"doc_urls":["index.html#welcome","tools.html#spectrum"]}')).toBe(
      '{"doc_urls":["./#welcome","tools#spectrum"]}',
    );
  });

  it("leaves other sites, assets and code alone", () => {
    const untouched = [
      '<a href="https://www.gnu.org/licenses/agpl-3.0.html">',
      '<a href="//example.com/page.html">',
      '<link href="../css/general-e96d0476.css">',
      "current_page += 'index.html';",
      "<code>&quot;page.html&quot;</code>",
    ];
    for (const text of untouched) {
      expect(cleanLinks(text)).toBe(text);
    }
  });
});
