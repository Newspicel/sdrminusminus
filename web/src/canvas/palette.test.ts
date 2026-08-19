import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, PatchCatalog } from "../lib/types";
import { channelPicker, filterPalette, firstPaletteItem, paletteGroups } from "./palette";

const CATALOG: PatchCatalog = {
  nodes: [
    { kind: "device", name: "Device", category: "source", ports: [] },
    { kind: "gps", name: "GPS position", category: "source", ports: [] },
    { kind: "channel", name: "Channel", category: "channel", ports: [], needs_channel_type: true },
    { kind: "scope", name: "Scope", category: "display", ports: [] },
    { kind: "speaker", name: "Speaker", category: "sink", ports: [] },
    { kind: "event_output", name: "Event output", category: "sink", ports: [] },
    { kind: "scanner", name: "Scanner", category: "feature", ports: [] },
  ],
};

const TYPES: ChannelDescriptor[] = [
  {
    type_id: "nfm",
    name: "NFM",
    bandwidth_hz: 12_500,
    input_rate_hz: 48_000,
    has_audio: true,
    exact_rate_only: false,
  },
  {
    type_id: "adsb",
    name: "ADS-B (1090ES)",
    bandwidth_hz: 2_000_000,
    input_rate_hz: 2_000_000,
    has_audio: false,
    decoder_kind: "adsb",
    exact_rate_only: false,
  },
];

describe("paletteGroups", () => {
  it("orders the sections and splits channels by what they produce", () => {
    const groups = paletteGroups(CATALOG, TYPES);
    expect(groups.map((group) => group.title)).toEqual([
      "Sources",
      "Modes",
      "Decoders",
      "Displays",
      "Sinks",
      "Tools",
    ]);
    expect(groups[1]?.items).toEqual([
      { id: "channel:nfm", name: "NFM", kind: "channel", type: TYPES[0] },
    ]);
    expect(groups[2]?.items[0]?.id).toBe("channel:adsb");
    expect(groups[4]?.items.map((item) => item.id)).toContain("event_output");
    expect(groups[0]?.items.map((item) => item.id)).toEqual(["device", "gps:gpsd", "gps:nmea"]);
  });

  it("offers device GPS only when this WebView exposes geolocation", () => {
    const groups = paletteGroups(CATALOG, TYPES, true);
    expect(groups[0]?.items.map((item) => item.id)).toContain("gps:device");
  });

  it("drops a section the server describes nothing for", () => {
    const groups = paletteGroups({ nodes: [CATALOG.nodes[0]!] }, []);
    expect(groups.map((group) => group.id)).toEqual(["source"]);
  });
});

describe("channelPicker", () => {
  it("pins the suggested mode above the full split list", () => {
    const groups = channelPicker(TYPES, "adsb");
    expect(groups.map((group) => group.title)).toEqual(["Suggested", "Modes", "Decoders"]);
    expect(groups[0]?.items).toEqual([
      { id: "suggested:adsb", name: "ADS-B (1090ES)", kind: "channel", type: TYPES[1] },
    ]);
  });

  it("offers every type on a server that does not describe the suggested one", () => {
    expect(channelPicker(TYPES, "atv").map((group) => group.id)).toEqual(["mode", "decoder"]);
  });

  it("drops the half of the list the server offers nothing for", () => {
    expect(channelPicker([TYPES[0]!], "nfm").map((group) => group.id)).toEqual([
      "suggested",
      "mode",
    ]);
  });
});

describe("firstPaletteItem", () => {
  it("takes the suggestion when one is pinned", () => {
    expect(firstPaletteItem(channelPicker(TYPES, "adsb"))?.id).toBe("suggested:adsb");
  });

  it("takes the top of the narrowed list when a search has moved past it", () => {
    const groups = filterPalette(channelPicker(TYPES, "adsb"), "nfm");
    expect(firstPaletteItem(groups)?.type?.type_id).toBe("nfm");
  });

  it("has nothing to take from a list that matches nothing", () => {
    expect(firstPaletteItem(filterPalette(channelPicker(TYPES, "nfm"), "atv"))).toBeUndefined();
  });
});

describe("filterPalette", () => {
  const groups = paletteGroups(CATALOG, TYPES);

  it("keeps everything for an empty query", () => {
    expect(filterPalette(groups, "  ")).toEqual(groups);
  });

  it("matches the type id an operator already knows, not just the display name", () => {
    const hits = filterPalette(groups, "adsb");
    expect(hits).toHaveLength(1);
    expect(hits[0]?.items.map((item) => item.name)).toEqual(["ADS-B (1090ES)"]);
  });

  it("matches names case-insensitively and drops emptied sections", () => {
    const hits = filterPalette(groups, "sc");
    expect(hits.map((group) => group.title)).toEqual(["Displays", "Tools"]);
  });
});
