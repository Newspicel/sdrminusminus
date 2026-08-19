import { describe, expect, it } from "vitest";
import { blankStyle, chooseBasemap, OFFLINE_BASEMAP_URL, offlineStyle } from "./basemap";

const BACKGROUND = "#101113";
const INK = "#e8e8ea";
const LINE = "#2a2c31";

describe("chooseBasemap", () => {
  it("takes the online style whenever one came back", () => {
    const online = blankStyle("#000");
    const chosen = chooseBasemap(online, true, BACKGROUND, INK, LINE);
    expect(chosen.kind).toBe("online");
    expect(chosen.style).toBe(online);
  });

  it("falls back to the operator's own archive", () => {
    const chosen = chooseBasemap(null, true, BACKGROUND, INK, LINE);
    expect(chosen.kind).toBe("offline");
    expect(JSON.stringify(chosen.style)).toContain(`pmtiles://${OFFLINE_BASEMAP_URL}`);
  });

  it("says plainly when there is nothing to draw", () => {
    const chosen = chooseBasemap(null, false, BACKGROUND, INK, LINE);
    expect(chosen.kind).toBe("blank");
    expect(chosen.style.layers).toHaveLength(1);
  });
});

describe("offlineStyle", () => {
  it("names only layers every ordinary extract carries", () => {
    const style = offlineStyle(BACKGROUND, INK, LINE);
    const layers = style.layers.map((layer) =>
      "source-layer" in layer ? layer["source-layer"] : null,
    );
    expect(layers).toContain("water");
    expect(layers).toContain("transportation");
    expect(style.sources.basemap).toEqual({
      type: "vector",
      url: `pmtiles://${OFFLINE_BASEMAP_URL}`,
    });
  });
});
