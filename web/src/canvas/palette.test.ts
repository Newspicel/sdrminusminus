import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, NodeKind, PatchCatalog } from "../lib/types";
import {
  channelPicker,
  decoderGroups,
  filterPalette,
  firstPaletteItem,
  paletteGroups,
  sectionGroups,
} from "./palette";

const CATALOG: PatchCatalog = {
  nodes: [
    { kind: "device", name: "Device", summary: "A radio", category: "source", ports: [] },
    { kind: "array", name: "Array", category: "tool", ports: [] },
    { kind: "gps", name: "GPS position", category: "source", ports: [] },
    { kind: "channel", name: "Channel", category: "channel", ports: [], needs_channel_type: true },
    { kind: "scope", name: "Scope", category: "output", ports: [] },
    { kind: "speaker", name: "Speaker", category: "output", ports: [] },
    { kind: "event_output", name: "Event output", category: "output", ports: [] },
    { kind: "scanner", name: "Scanner", category: "tool", ports: [] },
    { kind: "df", name: "Direction finder", category: "tool", ports: [] },
    { kind: "passive_radar", name: "Passive radar", category: "tool", ports: [] },
    { kind: "combiner", name: "Combiner", category: "tool", ports: [] },
  ],
};

const TYPES: ChannelDescriptor[] = [
  {
    type_id: "nfm",
    name: "NFM",
    summary: "Narrowband FM voice",
    family: "analog_voice",
    bandwidth_hz: 12_500,
    input_rate_hz: 48_000,
    has_audio: true,
  },
  {
    type_id: "adsb",
    name: "ADS-B (1090ES)",
    summary: "Aircraft positions",
    family: "aviation",
    bandwidth_hz: 2_000_000,
    input_rate_hz: 2_400_000,
    has_audio: false,
    decoder_kind: "adsb",
  },
];

describe("paletteGroups", () => {
  it("orders the sections and splits decoders by family", () => {
    const groups = paletteGroups(CATALOG, TYPES);
    expect(groups.map((group) => group.title)).toEqual([
      "Sources",
      "Analog voice",
      "Aviation",
      "Tools",
      "Outputs",
    ]);
    expect(groups[1]?.items).toEqual([
      {
        id: "channel:nfm",
        name: "NFM",
        summary: "Narrowband FM voice",
        kind: "channel",
        type: TYPES[0],
      },
    ]);
    expect(groups[3]?.items.map((item) => item.id)).toEqual([
      "array",
      "scanner",
      "df",
      "passive_radar",
      "combiner",
    ]);
    expect(groups[4]?.items.map((item) => item.id)).toContain("event_output");
    expect(groups[0]?.items.map((item) => item.id)).toEqual(["device", "gps"]);
  });

  it("carries the server's summary", () => {
    expect(paletteGroups(CATALOG, TYPES)[0]?.items[0]?.summary).toBe("A radio");
  });

  it("drops a section the server describes nothing for", () => {
    const groups = paletteGroups({ nodes: [CATALOG.nodes[0]!] }, TYPES);
    expect(groups.map((group) => group.id)).toEqual(["source"]);
  });
});

describe("decoderGroups", () => {
  it("files a type without a family under utility", () => {
    const { family: _family, ...bare } = TYPES[0]!;
    expect(decoderGroups([bare]).map((group) => group.title)).toEqual(["Utility"]);
  });
});

describe("sectionGroups", () => {
  it("keeps every family of one section", () => {
    const groups = sectionGroups(paletteGroups(CATALOG, TYPES), "channel");
    expect(groups.map((group) => group.id)).toEqual(["family:analog_voice", "family:aviation"]);
  });
});

describe("channelPicker", () => {
  it("pins the suggested mode above the full list", () => {
    const groups = channelPicker(TYPES, "adsb");
    expect(groups.map((group) => group.title)).toEqual(["Suggested", "Analog voice", "Aviation"]);
    expect(groups[0]?.items.map((item) => item.id)).toEqual(["suggested:adsb"]);
  });

  it("offers every type on a server that does not describe the suggested one", () => {
    expect(channelPicker(TYPES, "atv").map((group) => group.section)).toEqual([
      "channel",
      "channel",
    ]);
  });

  it("has only the suggestion to show when the server describes one type", () => {
    expect(channelPicker([TYPES[0]!], "nfm").map((group) => group.id)).toEqual([
      "suggested",
      "family:analog_voice",
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

  it("matches what a node does, not only its name", () => {
    const hits = filterPalette(groups, "aircraft");
    expect(hits.flatMap((group) => group.items).map((item) => item.id)).toEqual(["channel:adsb"]);
  });

  it("matches names case-insensitively and drops emptied sections", () => {
    const hits = filterPalette(groups, "sc");
    expect(hits.map((group) => group.title)).toEqual(["Tools", "Outputs"]);
  });

  it("offers every node the server describes, whatever its category", () => {
    const offered = new Set(
      paletteGroups(CATALOG, TYPES)
        .flatMap((group) => group.items)
        .map((item) => item.kind),
    );
    for (const entry of CATALOG.nodes) {
      expect(offered.has(entry.kind as NodeKind), `${entry.kind} is not in the palette`).toBe(true);
    }
  });
});
