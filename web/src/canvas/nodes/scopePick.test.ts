import { describe, expect, it } from "vitest";
import { FULL_VIEW } from "../../components/spectrumView";
import type { BandPlan, ChannelInfo } from "../../lib/types";
import {
  bookmarkDraft,
  channelTypeAt,
  pickAt,
  pickText,
  scopeSource,
  takeCreationTune,
  tuneOnCreate,
} from "./scopePick";

const PLAN: BandPlan = {
  region: { id: "de", name: "Germany", itu_region: "r1", layers: ["world"] },
  layers: [
    {
      id: "world",
      name: "ITU world table",
      authority: "ITU",
      kind: "world",
      rank: 0,
      source: "RR 2020",
    },
  ],
  allocations: [
    {
      id: "marine",
      layer: "world",
      name: "Marine VHF",
      official_name: "MARITIME MOBILE",
      service: "maritime",
      start_hz: 156_000_000,
      stop_hz: 162_000_000,
      suggested: { type: "nfm", settings: {} },
    },
  ],
  lanes: [
    {
      id: "allocation",
      name: "Allocation",
      overlay: false,
      blocks: [{ of: 0, start_hz: 156_000_000, stop_hz: 162_000_000 }],
    },
  ],
};

function channel(params: ChannelInfo["settings"]["params"]): ChannelInfo {
  return {
    id: 1,
    stream: 0,
    settings: { offset_hz: 0, squelch_db: null, params },
  };
}

describe("scopeSource", () => {
  it("shows the radio when nothing has been picked, tap or no tap", () => {
    expect(scopeSource("iq", true, true)).toBe("iq");
    expect(scopeSource("iq", true, false)).toBe("iq");
  });

  it("shows the channel tap once the operator picks it", () => {
    expect(scopeSource("baseband", true, true)).toBe("baseband");
  });

  it("falls back to the wire that is left rather than blanking the face", () => {
    expect(scopeSource("iq", false, true)).toBe("baseband");
    expect(scopeSource("baseband", true, false)).toBe("iq");
  });

  it("stays on the spectrum when neither wire is drawn", () => {
    expect(scopeSource("baseband", false, false)).toBe("iq");
    expect(scopeSource("iq", false, false)).toBe("iq");
  });
});

describe("pickAt", () => {
  it("reads the centre at the middle of a full view", () => {
    expect(pickAt(100_000_000, 2_000_000, FULL_VIEW, 0.5)).toEqual({
      hz: 100_000_000,
      offsetHz: 0,
    });
  });

  it("reads the edges of the span", () => {
    expect(pickAt(100_000_000, 2_000_000, FULL_VIEW, 0)).toEqual({
      hz: 99_000_000,
      offsetHz: -1_000_000,
    });
    expect(pickAt(100_000_000, 2_000_000, FULL_VIEW, 1)).toEqual({
      hz: 101_000_000,
      offsetHz: 1_000_000,
    });
  });

  it("reads through a zoomed window", () => {
    expect(pickAt(100_000_000, 2_000_000, { start: 0.5, end: 0.75 }, 0.5)).toEqual({
      hz: 100_250_000,
      offsetHz: 250_000,
    });
  });
});

describe("pickText", () => {
  it("carries the unit so a paste back into a dial means hertz", () => {
    expect(pickText({ hz: 156_800_000, offsetHz: 12_500 })).toEqual({
      frequency: "156800000 Hz",
      offset: "+12500 Hz",
    });
  });

  it("signs an offset below the centre with an ASCII minus", () => {
    expect(pickText({ hz: 99_488_000, offsetHz: -512_000 }).offset).toBe("-512000 Hz");
  });

  it("rounds to whole hertz", () => {
    expect(pickText({ hz: 100_000_000.4, offsetHz: -0.4 })).toEqual({
      frequency: "100000000 Hz",
      offset: "+0 Hz",
    });
  });
});

describe("bookmarkDraft", () => {
  it("names the allocation the frequency falls in, with the mode it suggests", () => {
    expect(bookmarkDraft(156_800_000, PLAN)).toEqual({ label: "Marine VHF", mode: "nfm" });
  });

  it("falls back to the frequency itself where nothing is allocated", () => {
    expect(bookmarkDraft(140_000_000, PLAN)).toEqual({ label: "140.0000 MHz", mode: null });
  });

  it("works with no band plan loaded", () => {
    expect(bookmarkDraft(156_800_000, null)).toEqual({ label: "156.8000 MHz", mode: null });
  });
});

describe("channelTypeAt", () => {
  it("prefers what the band plan allocates there", () => {
    expect(
      channelTypeAt({ type: "am", settings: {} }, channel({ type: "ssb", settings: {} })),
    ).toBe("am");
  });

  it("otherwise opens in the mode already being listened to", () => {
    expect(channelTypeAt(null, channel({ type: "ssb", settings: {} }))).toBe("ssb");
  });

  it("falls back to narrow FM", () => {
    expect(channelTypeAt(null, undefined)).toBe("nfm");
  });
});

describe("creation tunes", () => {
  it("hands the offset back once and then forgets it", () => {
    tuneOnCreate("channel:abc", 12_500);
    expect(takeCreationTune("channel:abc")).toBe(12_500);
    expect(takeCreationTune("channel:abc")).toBeUndefined();
  });

  it("knows nothing about a node that was not drawn at a frequency", () => {
    expect(takeCreationTune("channel:never")).toBeUndefined();
  });
});
