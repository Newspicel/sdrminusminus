const NOT_FOUND = "/404.html";

export function pagePath(pathname: string): string | null {
  const trimmed = pathname.replace(/\/+$/, "");
  if (trimmed === "" || /\.[^/]*$/.test(trimmed)) {
    return null;
  }
  return `${trimmed}.html`;
}

async function page(url: URL, env: Env): Promise<URL | null> {
  const path = pagePath(url.pathname);
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
  const target = await page(url, env);
  if (target !== null) {
    return Response.redirect(target.href, 301);
  }
  const notFound = await env.ASSETS.fetch(new URL(NOT_FOUND, url));
  return new Response(notFound.body, { status: 404, headers: notFound.headers });
}
