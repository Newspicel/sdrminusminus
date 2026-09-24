import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ABOUT_KEY } from "../lib/api";
import type { AboutResponse } from "../lib/types";
import { FieldPanel } from "./FieldPanel";

function render(about: Partial<AboutResponse>): string {
  const client = new QueryClient();
  client.setQueryData(ABOUT_KEY, about);
  return renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <FieldPanel />
    </QueryClientProvider>,
  );
}

describe("FieldPanel", () => {
  beforeEach(() => {
    vi.stubGlobal("window", { location: { origin: "http://127.0.0.1:8080" } });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("links to a LAN address when a phone can reach the server", () => {
    expect(render({ lan_addresses: ["192.168.1.20"] })).toContain("http://192.168.1.20:8080/field");
  });

  it("says how to open a loopback server instead of a link", () => {
    const html = render({ local_only: true });
    expect(html).not.toContain("/field");
    expect(html).toContain("--bind 0.0.0.0:8080");
  });
});
