import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, EventKindFacets } from "../../lib/types";
import {
  facetsOf,
  filterSaid,
  formatIds,
  fromTriState,
  kindsOffered,
  MAX_FILTER_IDS,
  parseIds,
  parseWords,
  predicatesFor,
  sectionsFor,
  stationLabel,
  toTriState,
} from "./eventFilter";

const FACETS: EventKindFacets[] = [
  { kind: "adsb", facets: ["position"] },
  { kind: "ais", facets: ["position"] },
  { kind: "call", facets: ["voice", "duration"] },
  { kind: "dv", facets: ["position", "voice"] },
];

describe("parseIds", () => {
  it("takes commas, spaces and newlines alike", () => {
    expect(parseIds("505, 9\n77  1")).toEqual([505, 9, 77, 1]);
  });

  it("drops anything that is not a whole non-negative id", () => {
    expect(parseIds("505, abc, -3, 1.5, , 9")).toEqual([505, 9]);
  });

  it("keeps each id once", () => {
    expect(parseIds("505 505 505")).toEqual([505]);
  });

  it("stops at the limit the server enforces", () => {
    const many = Array.from({ length: MAX_FILTER_IDS + 50 }, (_, i) => i).join(",");
    expect(parseIds(many)).toHaveLength(MAX_FILTER_IDS);
  });

  it("round-trips through the text field", () => {
    expect(parseIds(formatIds([505, 9]))).toEqual([505, 9]);
  });
});

describe("tri-state flags", () => {
  it("maps an unset flag to any and back", () => {
    expect(toTriState(null)).toBe("any");
    expect(toTriState(undefined)).toBe("any");
    expect(fromTriState("any")).toBeUndefined();
  });

  it("maps both settled states", () => {
    expect(toTriState(true)).toBe("yes");
    expect(toTriState(false)).toBe("no");
    expect(fromTriState("yes")).toBe(true);
    expect(fromTriState("no")).toBe(false);
  });
});

describe("kindsOffered", () => {
  const descriptors = [
    { type_id: "adsb", decoder_kind: "adsb" },
    { type_id: "dmr", decoder_kind: "dv" },
    { type_id: "pocsag", decoder_kind: "pocsag" },
    { type_id: "am", decoder_kind: null },
  ] as ChannelDescriptor[];

  it("offers only what the wired decoders emit", () => {
    expect(
      kindsOffered([{ channelType: "adsb", recordsCalls: false, trunk: false }], descriptors),
    ).toEqual(["adsb"]);
  });

  it("adds calls only when the channel records them", () => {
    expect(
      kindsOffered([{ channelType: "dmr", recordsCalls: false, trunk: false }], descriptors),
    ).toEqual(["dv"]);
    expect(
      kindsOffered([{ channelType: "dmr", recordsCalls: true, trunk: false }], descriptors),
    ).toEqual(["call", "dv"]);
  });

  it("treats a trunk system as digital voice", () => {
    expect(kindsOffered([{ recordsCalls: true, trunk: true }], descriptors)).toEqual([
      "call",
      "dv",
    ]);
  });

  it("offers nothing for a channel that decodes nothing", () => {
    expect(
      kindsOffered([{ channelType: "am", recordsCalls: false, trunk: false }], descriptors),
    ).toEqual([]);
  });

  it("lists each kind once across several wires", () => {
    expect(
      kindsOffered(
        [
          { channelType: "adsb", recordsCalls: false, trunk: false },
          { channelType: "adsb", recordsCalls: false, trunk: false },
          { channelType: "pocsag", recordsCalls: false, trunk: false },
        ],
        descriptors,
      ),
    ).toEqual(["adsb", "pocsag"]);
  });
});

describe("predicatesFor", () => {
  it("offers an aircraft wire no talkgroups", () => {
    const shown = predicatesFor(["adsb"], FACETS);
    expect(shown).toEqual(["stations", "contains", "has_position"]);
    expect(shown).not.toContain("talkgroups");
    expect(shown).not.toContain("encrypted");
  });

  it("offers a pager wire neither talkgroups nor position", () => {
    expect(predicatesFor(["pocsag"], FACETS)).toEqual(["stations", "contains"]);
  });

  it("offers a call wire the voice predicates and a duration", () => {
    expect(predicatesFor(["call"], FACETS)).toEqual([
      "stations",
      "contains",
      "talkgroups",
      "radios",
      "encrypted",
      "emergency",
      "min_duration_ms",
    ]);
  });

  it("offers a raw voice wire no duration, since a frame has none", () => {
    expect(predicatesFor(["dv"], FACETS)).not.toContain("min_duration_ms");
    expect(predicatesFor(["dv"], FACETS)).toContain("talkgroups");
  });

  it("offers the union across a mixed wire", () => {
    const shown = predicatesFor(["adsb", "call"], FACETS);
    expect(shown).toContain("has_position");
    expect(shown).toContain("talkgroups");
  });
});

describe("stationLabel", () => {
  it("names what the wire actually carries", () => {
    expect(stationLabel(["adsb"], FACETS)).toBe("Aircraft");
    expect(stationLabel(["ais"], FACETS)).toBe("Vessels");
    expect(stationLabel(["call", "dv"], FACETS)).toBe("Radios seen");
    expect(stationLabel(["adsb", "pocsag"], FACETS)).toBe("Stations");
    expect(stationLabel([], FACETS)).toBe("Stations");
  });
});

describe("parseWords", () => {
  it("splits on commas and spaces and keeps each once", () => {
    expect(parseWords("BAW890, RYR9AB  BAW890")).toEqual(["BAW890", "RYR9AB"]);
  });

  it("drops empties", () => {
    expect(parseWords(" , ,  ")).toEqual([]);
  });
});

describe("filterSaid", () => {
  it("says it keeps everything when nothing is set", () => {
    expect(filterSaid({})).toBe("keep every event");
  });

  it("names each predicate that is set", () => {
    const said = filterSaid({
      kinds: ["call"],
      talkgroups: [505],
      radios: [1001],
      encrypted: false,
      emergency: true,
      min_duration_ms: 1_500,
    });
    expect(said).toBe("keep call · TG 505 · radio 1001 · clear · emergency · over 1.5 s");
  });

  it("leads with drop when the filter removes what matches", () => {
    expect(filterSaid({ mode: "drop", kinds: ["pocsag"], contains: "TEST" })).toBe(
      'drop pocsag · "TEST"',
    );
  });

  it("admits that an empty drop filter drops nothing", () => {
    expect(filterSaid({ mode: "drop" })).toBe("drop nothing");
  });
});

describe("facetsOf", () => {
  it("reads the facets the server declared for a kind", () => {
    expect(facetsOf("call", FACETS)).toEqual(["voice", "duration"]);
    expect(facetsOf("pocsag", FACETS)).toEqual([]);
  });
});

const titles = (kinds: string[]) => sectionsFor(kinds, FACETS).map((s) => s.title);

describe("sectionsFor", () => {
  it("gives an aircraft wire only the sections that apply", () => {
    expect(titles(["adsb"])).toEqual(["Any event", "Position"]);
  });

  it("gives a pager wire one section", () => {
    expect(titles(["pocsag"])).toEqual(["Any event"]);
    expect(sectionsFor(["pocsag"], FACETS)[0]?.predicates).toEqual(["stations", "contains"]);
  });

  it("gives a call wire the voice section", () => {
    expect(titles(["call"])).toEqual(["Any event", "Voice"]);
    expect(sectionsFor(["call"], FACETS).at(-1)?.predicates).toEqual([
      "talkgroups",
      "radios",
      "encrypted",
      "emergency",
      "min_duration_ms",
    ]);
  });

  it("narrows when a kind is picked out of a mixed wire", () => {
    expect(titles(["adsb", "call"])).toEqual(["Any event", "Position", "Voice"]);
    expect(titles(["adsb"])).toEqual(["Any event", "Position"]);
    expect(titles(["call"])).toEqual(["Any event", "Voice"]);
  });

  it("names the kinds each section judges, and nothing more", () => {
    const mixed = sectionsFor(["adsb", "call"], FACETS);
    expect(mixed.map((s) => [s.title, s.applies])).toEqual([
      ["Any event", ["adsb", "call"]],
      ["Position", ["adsb"]],
      ["Voice", ["call"]],
    ]);
  });
});
