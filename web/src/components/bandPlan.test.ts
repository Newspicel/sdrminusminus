import { describe, expect, it } from "vitest";
import type { BandAllocation, BandBlock, BandLane, BandPlan } from "../lib/types";
import {
  identify,
  parseFrequency,
  searchPlan,
  serviceLabel,
  spansIn,
  suggestedAt,
} from "./bandPlan";

function allocation(over: Partial<BandAllocation> & Pick<BandAllocation, "id">): BandAllocation {
  return {
    layer: "world",
    start_hz: 0,
    stop_hz: 1,
    service: "other",
    name: "",
    aliases: [],
    ...over,
  };
}

function block(startHz: number, stopHz: number, over: Partial<BandAllocation>): BandBlock {
  return {
    start_hz: startHz,
    stop_hz: stopHz,
    allocation: allocation({
      id: `${over.name ?? "x"}:${startHz}`,
      start_hz: startHz,
      stop_hz: stopHz,
      ...over,
    }),
  };
}

const MARINE = block(156_000_000, 161_962_500, {
  name: "Marine VHF",
  service: "maritime",
  aliases: ["marine vhf", "channel 16"],
});
const AIS = block(161_962_500, 162_037_500, {
  name: "AIS",
  service: "maritime",
  aliases: ["ais", "ship tracking"],
});
const TWO_M = block(144_000_000, 146_000_000, {
  name: "2 m amateur",
  service: "amateur",
  aliases: ["2 m", "70 cm"],
  suggested: { type: "nfm", settings: {} },
});

const ALLOCATION: BandLane = {
  id: "allocation",
  name: "Allocation",
  overlay: false,
  blocks: [TWO_M, MARINE, AIS],
};
const AMATEUR: BandLane = {
  id: "iaru-r1",
  name: "Amateur band plan — IARU R1",
  overlay: true,
  blocks: [
    block(144_794_000, 144_990_000, {
      name: "2 m — APRS",
      service: "amateur",
      aliases: ["aprs"],
      suggested: { type: "aprs", settings: {} },
    }),
    block(145_206_000, 145_594_000, {
      name: "2 m — FM simplex",
      service: "amateur",
      aliases: ["simplex"],
    }),
  ],
};
const PLAN: BandPlan = {
  region: {
    id: "de",
    name: "Germany",
    itu_region: "r1",
    layers: ["world"],
  },
  layers: [
    {
      id: "world",
      name: "ITU world table",
      authority: "ITU",
      source: "RR Article 5",
      kind: "world",
      rank: 0,
    },
  ],
  lanes: [ALLOCATION, AMATEUR],
};

describe("spansIn", () => {
  it("clips a block that runs off both edges and says which edges are real", () => {
    // A 1 MHz window entirely inside the 2 m band.
    const [span] = spansIn(ALLOCATION, 144_500_000, 1_000_000);
    expect(span?.left).toBe(0);
    expect(span?.width).toBe(1);
    expect(span?.startsInside).toBe(false);
    expect(span?.endsInside).toBe(false);
  });

  it("places a band that sits wholly inside the window", () => {
    const spans = spansIn(ALLOCATION, 161_900_000, 200_000);
    const ais = spans.find((span) => span.block.allocation.name === "AIS");
    expect(ais?.left).toBeCloseTo(0.3125, 10);
    expect(ais?.width).toBeCloseTo(0.375, 10);
    expect(ais?.startsInside).toBe(true);
    expect(ais?.endsInside).toBe(true);
  });

  it("drops what the window does not reach, including a block that only touches its edge", () => {
    expect(spansIn(ALLOCATION, 100_000_000, 1_000_000)).toEqual([]);
    // The window ends exactly where 2 m begins: a half-open block starting there is not visible.
    expect(spansIn(ALLOCATION, 143_000_000, 1_000_000)).toEqual([]);
  });

  it("returns nothing for a zero or negative window rather than dividing by it", () => {
    expect(spansIn(ALLOCATION, 144_000_000, 0)).toEqual([]);
    expect(spansIn(ALLOCATION, 144_000_000, -1)).toEqual([]);
  });
});

describe("identify", () => {
  it("answers once per lane that covers the frequency, in lane order", () => {
    const found = identify(PLAN, 145_500_000);
    expect(found.map((entry) => entry.laneId)).toEqual(["allocation", "iaru-r1"]);
    expect(found[0]?.block.allocation.name).toBe("2 m amateur");
    expect(found[1]?.block.allocation.name).toBe("2 m — FM simplex");
  });

  it("omits a lane with nothing there rather than reporting it empty", () => {
    const found = identify(PLAN, 156_800_000);
    expect(found).toHaveLength(1);
    expect(found[0]?.laneId).toBe("allocation");
  });

  it("treats a block as half-open, so a boundary belongs to the band above it", () => {
    expect(identify(PLAN, 161_962_500)[0]?.block.allocation.name).toBe("AIS");
    expect(identify(PLAN, 161_962_499)[0]?.block.allocation.name).toBe("Marine VHF");
  });

  it("has no answer outside every band", () => {
    expect(identify(PLAN, 1)).toEqual([]);
  });
});

describe("suggestedAt", () => {
  it("takes the most specific lane's mode, which is the whole point of the overlay", () => {
    // 144.800 MHz: the allocation says "2 m amateur, NFM", the IARU plan says APRS. Reading the
    // first lane instead of the last would tune the headline band and miss the sub-band —
    // exactly the case the amateur overlay exists for.
    expect(suggestedAt(identify(PLAN, 144_800_000))?.type).toBe("aprs");
  });

  it("falls back through lanes that suggest nothing", () => {
    // 145.5 MHz is in the FM-simplex segment, which carries no mode of its own.
    expect(suggestedAt(identify(PLAN, 145_500_000))?.type).toBe("nfm");
  });

  it("has nothing to suggest where nothing is allocated", () => {
    expect(suggestedAt(identify(PLAN, 1))).toBeNull();
    expect(suggestedAt(identify(PLAN, 156_800_000))).toBeNull();
  });
});

describe("searchPlan", () => {
  it("ignores filler words instead of failing on them", () => {
    const hits = searchPlan(PLAN, "show me marine VHF");
    expect(hits[0]?.allocation.name).toBe("Marine VHF");
  });

  it("finds the amateur service by the word half the world uses for it", () => {
    const hits = searchPlan(PLAN, "70 cm ham");
    expect(hits.map((hit) => hit.allocation.name)).toContain("2 m amateur");
  });

  it("resolves a query that reads as a frequency, and ranks it above a name match", () => {
    const hits = searchPlan(PLAN, "145.5");
    // Both lanes cover it; the narrower band plan entry wins the tie between two frequency hits.
    expect(hits[0]?.allocation.name).toBe("2 m — FM simplex");
    expect(hits[1]?.allocation.name).toBe("2 m amateur");
  });

  it("names every band only once even though a lane repeats it across blocks", () => {
    const split: BandPlan = {
      ...PLAN,
      lanes: [
        {
          ...ALLOCATION,
          blocks: [
            { ...MARINE, stop_hz: 158_000_000 },
            { ...MARINE, start_hz: 158_000_000 },
          ],
        },
      ],
    };
    expect(searchPlan(split, "marine")).toHaveLength(1);
  });

  it("is empty for a query with nothing long enough to match", () => {
    expect(searchPlan(PLAN, "")).toEqual([]);
    expect(searchPlan(PLAN, "a")).toEqual([]);
    expect(searchPlan(PLAN, "zzzz")).toEqual([]);
  });

  it("honours the limit", () => {
    expect(searchPlan(PLAN, "m", 1)).toHaveLength(0);
    expect(searchPlan(PLAN, "amateur", 1)).toHaveLength(1);
  });
});

describe("parseFrequency", () => {
  it("reads a bare number as megahertz, which is what the dial reads in", () => {
    expect(parseFrequency("145.5")).toBe(145_500_000);
    expect(parseFrequency("1090")).toBe(1_090_000_000);
  });

  it("reads every unit it is offered, in any case, with or without a space", () => {
    expect(parseFrequency("433 MHz")).toBe(433_000_000);
    expect(parseFrequency("77.5khz")).toBe(77_500);
    expect(parseFrequency("1.09 GHz")).toBeCloseTo(1_090_000_000, 3);
    expect(parseFrequency("9000 hz")).toBe(9_000);
  });

  it("accepts a comma decimal, which is how half of Europe types one", () => {
    expect(parseFrequency("145,500")).toBe(145_500_000);
  });

  it("refuses anything that is not only a number and a unit", () => {
    expect(parseFrequency("20 m")).toBeNull();
    expect(parseFrequency("marine vhf")).toBeNull();
    expect(parseFrequency("145.5 marine")).toBeNull();
    expect(parseFrequency("")).toBeNull();
  });
});

describe("serviceLabel", () => {
  it("spells an initialism as one and everything else as a word", () => {
    expect(serviceLabel("ism")).toBe("ISM");
    expect(serviceLabel("aeronautical")).toBe("Aeronautical");
  });
});
