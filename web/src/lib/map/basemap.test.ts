import { afterEach, describe, expect, it, vi } from "vitest";
import { clientEvents, resetEvents } from "../diagnostics";
import { blankStyle, chooseBasemap, fetchOnlineStyle } from "./basemap";

const BACKGROUND = "#101113";

describe("chooseBasemap", () => {
  it("takes the online style whenever one came back", () => {
    const online = blankStyle("#000");
    const chosen = chooseBasemap(online, BACKGROUND);
    expect(chosen.kind).toBe("online");
    expect(chosen.style).toBe(online);
  });

  it("says plainly when there is nothing to draw", () => {
    const chosen = chooseBasemap(null, BACKGROUND);
    expect(chosen.kind).toBe("blank");
    expect(chosen.style.layers).toHaveLength(1);
  });
});

describe("fetchOnlineStyle", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    resetEvents();
  });

  it("records why the online style is missing", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("", { status: 503 })));
    expect(await fetchOnlineStyle()).toBeNull();
    expect(clientEvents()).toContainEqual(
      expect.objectContaining({ level: "warn", source: "map", message: "basemap style: HTTP 503" }),
    );
  });
});
