import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, DecoderEvent } from "../../lib/types";
import {
  filterSaid,
  formatIds,
  fromTriState,
  kindsOffered,
  MAX_FILTER_IDS,
  parseIds,
  parseWords,
  passesChain,
  passesFilter,
  predicatesFor,
  sectionsFor,
  stationLabel,
  toTriState,
} from "./eventFilter";

const call = (over: Partial<Record<string, unknown>> = {}) =>
  ({
    kind: "call",
    data: {
      id: 1,
      node: "dmr",
      source_node: "dmr",
      started_at: "",
      ended_at: "",
      duration_ms: 2_000,
      device_set: 1,
      channel: 2,
      freq_hz: 446e6,
      mode: "dmr",
      source: 2_621_001,
      destination: 505,
      group_call: true,
      encrypted: false,
      emergency: false,
      ...over,
    },
  }) as DecoderEvent;

const voice = { kind: "dv", data: { mode: "dmr", kind: "voice" } } as DecoderEvent;
const rtty = { kind: "rtty", data: { text: "CQ" } } as DecoderEvent;

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
    const shown = predicatesFor(["adsb"]);
    expect(shown).toEqual(["stations", "contains", "has_position"]);
    expect(shown).not.toContain("talkgroups");
    expect(shown).not.toContain("encrypted");
  });

  it("offers a pager wire neither talkgroups nor position", () => {
    expect(predicatesFor(["pocsag"])).toEqual(["stations", "contains"]);
  });

  it("offers a call wire the voice predicates and a duration", () => {
    expect(predicatesFor(["call"])).toEqual([
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
    expect(predicatesFor(["dv"])).not.toContain("min_duration_ms");
    expect(predicatesFor(["dv"])).toContain("talkgroups");
  });

  it("offers the union across a mixed wire", () => {
    const shown = predicatesFor(["adsb", "call"]);
    expect(shown).toContain("has_position");
    expect(shown).toContain("talkgroups");
  });
});

describe("stationLabel", () => {
  it("names what the wire actually carries", () => {
    expect(stationLabel(["adsb"])).toBe("Aircraft");
    expect(stationLabel(["ais"])).toBe("Vessels");
    expect(stationLabel(["call", "dv"])).toBe("Radios seen");
    expect(stationLabel(["adsb", "pocsag"])).toBe("Stations");
    expect(stationLabel([])).toBe("Stations");
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
  it("says it passes everything when nothing is set", () => {
    expect(filterSaid({})).toBe("every event");
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
    expect(said).toBe("call · TG 505 · radio 1001 · clear · emergency · over 1.5 s");
  });
});

describe("passesFilter", () => {
  it("passes everything when nothing is named", () => {
    for (const event of [call(), voice, rtty]) {
      expect(passesFilter({}, event)).toBe(true);
    }
  });

  it("naming call keeps calls and drops raw voice frames", () => {
    const only = { kinds: ["call"] };
    expect(passesFilter(only, call())).toBe(true);
    expect(passesFilter(only, voice)).toBe(false);
    expect(passesFilter(only, rtty)).toBe(false);
  });

  it("matches the talkgroup, the radio and the flags", () => {
    expect(passesFilter({ talkgroups: [505] }, call())).toBe(true);
    expect(passesFilter({ talkgroups: [505] }, call({ destination: 9 }))).toBe(false);
    expect(passesFilter({ radios: [2_621_001] }, call())).toBe(true);
    expect(passesFilter({ radios: [1] }, call())).toBe(false);
    expect(passesFilter({ encrypted: true }, call())).toBe(false);
    expect(passesFilter({ emergency: false }, call())).toBe(true);
  });

  it("drops calls shorter than the floor", () => {
    expect(passesFilter({ min_duration_ms: 1_500 }, call())).toBe(true);
    expect(passesFilter({ min_duration_ms: 1_500 }, call({ duration_ms: 400 }))).toBe(false);
  });

  it("leaves other kinds alone when only call predicates are set", () => {
    expect(passesFilter({ talkgroups: [505], min_duration_ms: 60_000 }, rtty)).toBe(true);
  });

  it("needs every filter in a chain to agree", () => {
    const chain = [{ kinds: ["call"] }, { talkgroups: [505] }];
    expect(passesChain(chain, call())).toBe(true);
    expect(passesChain(chain, call({ destination: 9 }))).toBe(false);
    expect(passesChain(chain, voice)).toBe(false);
    expect(passesChain([], voice)).toBe(true);
  });
});

const adsbEvent = (icao: string, callsign?: string, fix = true) =>
  ({
    kind: "adsb",
    data: {
      icao,
      df: 17,
      raw: "",
      ...(callsign == null ? {} : { callsign }),
      ...(fix ? { lat: 50.4, lon: 6.6 } : {}),
    },
  }) as DecoderEvent;

describe("the generic predicates reach every kind", () => {
  const adsb = adsbEvent;

  it("matches an aircraft by its ICAO, whatever the case", () => {
    expect(passesFilter({ stations: ["3C6444"] }, adsb("3C6444"))).toBe(true);
    expect(passesFilter({ stations: ["3c6444"] }, adsb("3C6444"))).toBe(true);
    expect(passesFilter({ stations: ["3C6444"] }, adsb("4CA2D4"))).toBe(false);
  });

  it("searches the summary for a callsign", () => {
    expect(passesFilter({ contains: "baw" }, adsb("3C6444", "BAW890"))).toBe(true);
    expect(passesFilter({ contains: "baw" }, adsb("3C6444", "RYR9AB"))).toBe(false);
  });

  it("keeps only the aircraft that have a fix", () => {
    expect(passesFilter({ has_position: true }, adsb("3C6444", undefined, true))).toBe(true);
    expect(passesFilter({ has_position: true }, adsb("3C6444", undefined, false))).toBe(false);
  });

  it("leaves an aircraft alone when only voice predicates are set", () => {
    expect(passesFilter({ talkgroups: [505], encrypted: true }, adsb("3C6444"))).toBe(true);
  });

  it("applies the voice predicates to raw frames too", () => {
    const frame = {
      kind: "dv",
      data: { mode: "dmr", kind: "voice", destination: 505 },
    } as DecoderEvent;
    expect(passesFilter({ talkgroups: [505] }, frame)).toBe(true);
    expect(passesFilter({ talkgroups: [9] }, frame)).toBe(false);
    expect(passesFilter({ min_duration_ms: 5_000 }, frame)).toBe(true);
  });
});

const titles = (kinds: string[]) => sectionsFor(kinds).map((s) => s.title);

describe("sectionsFor", () => {
  it("gives an aircraft wire only the sections that apply", () => {
    expect(titles(["adsb"])).toEqual(["Any event", "Position"]);
  });

  it("gives a pager wire one section", () => {
    expect(titles(["pocsag"])).toEqual(["Any event"]);
    expect(sectionsFor(["pocsag"])[0]?.predicates).toEqual(["stations", "contains"]);
  });

  it("gives a call wire the voice section", () => {
    expect(titles(["call"])).toEqual(["Any event", "Voice"]);
    expect(sectionsFor(["call"]).at(-1)?.predicates).toEqual([
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
    const mixed = sectionsFor(["adsb", "call"]);
    expect(mixed.map((s) => [s.title, s.applies])).toEqual([
      ["Any event", ["adsb", "call"]],
      ["Position", ["adsb"]],
      ["Voice", ["call"]],
    ]);
  });
});
