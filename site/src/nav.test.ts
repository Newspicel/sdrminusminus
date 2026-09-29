import { describe, expect, it } from "vitest";
import summary from "../../docs/src/SUMMARY.md?raw";
import { COMMUNITY, FOOTER, isCurrent, LEGAL, PRIMARY } from "./nav";
import { canonical, docPages, sitePages } from "./seo";

const pages = Object.keys(import.meta.glob("./pages/*.astro"));
const known = new Set([...sitePages(pages), ...docPages(summary)]);

describe("links", () => {
  const links = [PRIMARY, COMMUNITY, LEGAL, ...FOOTER.map((group) => group.links)].flat();

  it("point at a page or a docs chapter", () => {
    const internal = links.filter((link) => link.href.startsWith("/"));
    expect(internal.length).toBeGreaterThan(0);
    for (const link of internal) {
      expect(known, link.href).toContain(canonical(link.href.replace(/#.*$/, "")).href);
    }
  });

  it("leave the site over HTTPS only", () => {
    for (const link of links.filter((candidate) => !candidate.href.startsWith("/"))) {
      expect(link.href).toMatch(/^https:\/\//);
    }
  });
});

describe("isCurrent", () => {
  it("matches with or without the extension", () => {
    expect(isCurrent("/download.html", "/download.html")).toBe(true);
    expect(isCurrent("/download.html", "/download")).toBe(true);
    expect(isCurrent("/download.html", "/download/")).toBe(true);
  });

  it("treats the index as the root", () => {
    expect(isCurrent("/", "/index.html")).toBe(true);
  });

  it("never marks other pages or external links", () => {
    expect(isCurrent("/remote.html", "/download.html")).toBe(false);
    expect(isCurrent("https://app.sdrmm.com", "/")).toBe(false);
  });
});
