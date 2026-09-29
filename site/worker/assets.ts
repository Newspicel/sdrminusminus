const NOT_FOUND = "/docs/404";
const DOCS = "/docs";

export function docsPath(pathname: string): string | null {
  const page = pathname.replace(/\/+$/, "").replace(/\.html$/, "");
  const inDocs = page === DOCS || page.startsWith(`${DOCS}/`);
  if (page === "" || inDocs || /\.[^/]*$/.test(page)) {
    return null;
  }
  return `${DOCS}${page}`;
}

async function movedToDocs(url: URL, env: Env): Promise<URL | null> {
  const path = docsPath(url.pathname);
  if (path === null) {
    return null;
  }
  const target = new URL(path, url);
  target.search = url.search;
  const found = await env.ASSETS.fetch(target, { method: "HEAD" });
  return found.ok ? target : null;
}

export async function missing(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const target = await movedToDocs(url, env);
  if (target !== null) {
    return Response.redirect(target.href, 301);
  }
  const notFound = await env.ASSETS.fetch(new URL(NOT_FOUND, url));
  return new Response(notFound.body, { status: 404, headers: notFound.headers });
}
