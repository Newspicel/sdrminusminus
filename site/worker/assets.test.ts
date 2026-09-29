import { describe, expect, it } from "vitest";
import { docsPath } from "./assets";
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

describe("docsPath", () => {
  it("maps old chapter addresses into the docs", () => {
    expect(docsPath("/hardware")).toBe("/docs/hardware");
    expect(docsPath("/hardware.html")).toBe("/docs/hardware");
    expect(docsPath("/user-guide/decoders/")).toBe("/docs/user-guide/decoders");
  });

  it("leaves the root, the docs and files alone", () => {
    expect(docsPath("/")).toBeNull();
    expect(docsPath("/docs")).toBeNull();
    expect(docsPath("/docs/nowhere")).toBeNull();
    expect(docsPath("/icon.svg")).toBeNull();
  });
});

describe("missing assets", () => {
  const env = environment({ "/docs/hardware": "hardware", "/docs/404": "not found" });

  it("moves an old chapter into the docs, keeping the query", async () => {
    const response = await get("/hardware.html?from=readme", env);
    expect(response.status).toBe(301);
    expect(response.headers.get("location")).toBe("https://sdrmm.com/docs/hardware?from=readme");
  });

  it("answers anything else with the 404 page", async () => {
    const response = await get("/nowhere", env);
    expect(response.status).toBe(404);
    expect(await response.text()).toBe("not found");
    expect(response.headers.get("content-type")).toBe("text/html");
  });
});
