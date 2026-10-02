export const SITE = "https://sdrmm.com";
export const NAME = "SDR--";
export const ALTERNATE_NAMES = ["SDRmm", "SDR minus minus", "sdrminusminus"];
export const REPOSITORY = "https://github.com/Newspicel/sdrmm";
export const SHARE_IMAGE = { path: "/og.png", width: 1200, height: 630 } as const;

export const DOCS = "/docs/";

const UNLISTED = new Set(["demo"]);

export function canonical(pathname: string): URL {
  return new URL(pathname.replace(/\.html$/, "").replace(/(^|\/)index$/, "$1"), SITE);
}

export function sitePages(sources: readonly string[]): string[] {
  return sources
    .map((source) => source.replace(/^.*\//, "").replace(/\.astro$/, ""))
    .filter((page) => !UNLISTED.has(page))
    .map((page) => canonical(`/${page}`).href);
}

export function docPages(summary: string): string[] {
  return [...summary.matchAll(/\]\(([^)#\s]+)\.md\)/g)].flatMap((match) =>
    match[1] === undefined ? [] : [canonical(`${DOCS}${match[1]}`).href],
  );
}

function escapeXml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function sitemap(urls: readonly string[]): string {
  const entries = [...new Set(urls)]
    .toSorted()
    .map((url) => `<url><loc>${escapeXml(url)}</loc></url>`)
    .join("");
  return `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${entries}</urlset>`;
}

export function robots(): string {
  return `User-agent: *\nAllow: /\n\nSitemap: ${new URL("/sitemap.xml", SITE).href}\n`;
}

export function structuredData(description: string) {
  const home = canonical("/").href;
  const names = { name: NAME, alternateName: ALTERNATE_NAMES };
  return {
    "@context": "https://schema.org",
    "@graph": [
      { "@type": "WebSite", "@id": `${home}#website`, url: home, ...names },
      {
        "@type": "SoftwareApplication",
        ...names,
        description,
        url: home,
        applicationCategory: "MultimediaApplication",
        operatingSystem: "Windows, macOS, Linux",
        downloadUrl: canonical("/download").href,
        image: canonical(SHARE_IMAGE.path).href,
        screenshot: canonical("/screens/patch.png").href,
        license: "https://www.gnu.org/licenses/agpl-3.0.html",
        isAccessibleForFree: true,
        offers: { "@type": "Offer", price: "0", priceCurrency: "EUR" },
        author: { "@type": "Person", name: "Julian Haag" },
        sameAs: [REPOSITORY],
      },
    ],
  };
}

export function jsonLd(data: unknown): string {
  return JSON.stringify(data).replace(/</g, "\\u003c");
}
