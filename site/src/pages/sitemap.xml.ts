import type { APIRoute } from "astro";
import summary from "../../../docs/src/SUMMARY.md?raw";
import { docPages, sitemap, sitePages } from "../seo";

const pages = Object.keys(import.meta.glob("./*.astro"));

export const GET: APIRoute = () =>
  new Response(sitemap([...sitePages(pages), ...docPages(summary)]), {
    headers: { "content-type": "application/xml" },
  });
