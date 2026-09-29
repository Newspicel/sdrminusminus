import { describe, expect, it } from "vitest";
import {
  canonical,
  docPages,
  jsonLd,
  robots,
  SITE,
  sitemap,
  sitePages,
  structuredData,
} from "./seo";

describe("canonical", () => {
  it("drops index.html so the home page has one address", () => {
    expect(canonical("/index.html").href).toBe(`${SITE}/`);
    expect(canonical("/").href).toBe(`${SITE}/`);
    expect(canonical("/docs/index.html").href).toBe(`${SITE}/docs/`);
  });

  it("drops page extensions", () => {
    expect(canonical("/download.html").href).toBe(`${SITE}/download`);
    expect(canonical("/download").href).toBe(`${SITE}/download`);
  });
});

describe("sitePages", () => {
  it("lists every page but the demo", () => {
    expect(sitePages(["./index.astro", "./download.astro", "./demo.astro"])).toEqual([
      `${SITE}/`,
      `${SITE}/download`,
    ]);
  });
});

describe("docPages", () => {
  it("lists the chapters of the book summary under the docs", () => {
    const summary =
      "[Welcome](index.md)\n\n# Get started\n\n- [Install](getting-started/install.md)\n- [Draft]()";
    expect(docPages(summary)).toEqual([`${SITE}/docs/`, `${SITE}/docs/getting-started/install`]);
  });
});

describe("sitemap", () => {
  it("writes each address once, sorted", () => {
    const xml = sitemap([`${SITE}/b`, `${SITE}/a`, `${SITE}/b`]);
    expect(xml).toContain('<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">');
    expect(xml.match(/<loc>/g)).toHaveLength(2);
    expect(xml.indexOf(`${SITE}/a`)).toBeLessThan(xml.indexOf(`${SITE}/b`));
  });

  it("escapes XML", () => {
    expect(sitemap([`${SITE}/?a=1&b=2`])).toContain("?a=1&amp;b=2");
  });
});

describe("robots", () => {
  it("points crawlers at the sitemap", () => {
    expect(robots()).toContain(`Sitemap: ${SITE}/sitemap.xml`);
  });
});

describe("structuredData", () => {
  it("names the site and the app with the spellings people search for", () => {
    const graph = structuredData("A radio.")["@graph"];
    for (const node of graph) {
      expect(node.alternateName).toContain("SDR minus minus");
    }
  });
});

describe("jsonLd", () => {
  it("cannot close the script element it sits in", () => {
    expect(jsonLd({ text: "</script><script>" })).not.toContain("</script>");
    expect(JSON.parse(jsonLd({ text: "</script>" }))).toEqual({ text: "</script>" });
  });
});
