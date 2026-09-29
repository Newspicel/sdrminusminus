import { describe, expect, it } from "vitest";
import { pagePath } from "./assets";
import worker from "./index";

function environment(pages: Record<string, string>) {
  return {
    ASSETS: {
      fetch: async (input: URL, init?: RequestInit) => {
        const body = pages[input.pathname];
        if (body === undefined) {
          return new Response("missing", { status: 404 });
        }
        return new Response(init?.method === "HEAD" ? null : body, {
          headers: { "content-type": "text/html" },
        });
      },
    },
  } as unknown as Env;
}

async function get(path: string, env: Env) {
  return worker.fetch(
    new Request(`https://sdrmm.com${path}`) as Parameters<typeof worker.fetch>[0],
    env,
  );
}

describe("pagePath", () => {
  it("maps extensionless paths to their page", () => {
    expect(pagePath("/download")).toBe("/download.html");
    expect(pagePath("/user-guide/decoders/")).toBe("/user-guide/decoders.html");
  });

  it("leaves the root and files alone", () => {
    expect(pagePath("/")).toBeNull();
    expect(pagePath("/icon.svg")).toBeNull();
    expect(pagePath("/download.html")).toBeNull();
  });
});

describe("missing assets", () => {
  const env = environment({ "/download.html": "download", "/404.html": "not found" });

  it("moves a page without its extension to the page, keeping the query", async () => {
    const response = await get("/download?from=readme", env);
    expect(response.status).toBe(301);
    expect(response.headers.get("location")).toBe("https://sdrmm.com/download.html?from=readme");
  });

  it("answers anything else with the 404 page", async () => {
    const response = await get("/nowhere", env);
    expect(response.status).toBe(404);
    expect(await response.text()).toBe("not found");
    expect(response.headers.get("content-type")).toBe("text/html");
  });
});
