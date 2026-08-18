import { describe, expect, it } from "vitest";
import type { TrunkChannel } from "../../lib/types";
import {
  adoptable,
  awaitingControlChannel,
  channelEntry,
  channelPlanRows,
  controlChannelStalled,
  DMR_TRUNK_PROTOCOLS,
  followsTierThree,
  formatSearchRanges,
  parseControlHz,
  parseSearchRanges,
  planLabel,
  planSummary,
  searchCandidates,
  searchSummary,
  trunkProtocolLabel,
  usable,
  withChannel,
  withoutChannel,
} from "./dmrTrunk";

describe("DMR trunk protocols", () => {
  it("offers Hytera XPT alongside the Motorola and Tier III systems", () => {
    expect(DMR_TRUNK_PROTOCOLS).toEqual([
      { value: "auto", label: "Auto-detect" },
      { value: "capacity_plus", label: "Capacity Plus" },
      { value: "hytera_xpt", label: "Hytera XPT" },
      { value: "tier_three", label: "Tier III / Capacity Max" },
    ]);
  });

  it("names the protocol it settled on rather than the one that was picked", () => {
    expect(trunkProtocolLabel("hytera_xpt")).toBe("Hytera XPT");
    expect(trunkProtocolLabel("auto", "capacity_plus")).toBe("Capacity Plus");
    expect(trunkProtocolLabel("auto", "tier_three")).toBe("Tier III");
    expect(trunkProtocolLabel("auto")).toBe("Listening for signalling");
  });

  it("calls the plan what it is for each kind of system", () => {
    expect(planLabel("tier_three")).toBe("Channel plan");
    expect(planLabel("auto", "tier_three")).toBe("Channel plan");
    expect(planLabel("capacity_plus")).toBe("Repeater outputs");
  });

  it("offers the search only where logical channels are granted", () => {
    expect(followsTierThree("tier_three")).toBe(true);
    expect(followsTierThree("auto", "tier_three")).toBe(true);
    expect(followsTierThree("auto", "capacity_plus")).toBe(false);
    expect(followsTierThree("capacity_plus")).toBe(false);
  });
});

describe("DMR channel plan", () => {
  it("takes a logical channel and a frequency in MHz", () => {
    expect(channelEntry(17, 451.0125)).toEqual({ lcn: 17, freq_hz: 451_012_500 });
  });

  it("waits for both halves before it makes an entry", () => {
    expect(channelEntry(17, null)).toBeNull();
    expect(channelEntry(null, 451.0125)).toBeNull();
  });

  it("refuses a channel or a frequency the system could never use", () => {
    expect(channelEntry(-1, 451.0125)).toBeNull();
    expect(channelEntry(99_999, 451.0125)).toBeNull();
    expect(channelEntry(17.5, 451.0125)).toBeNull();
    expect(channelEntry(17, 0)).toBeNull();
  });

  it("keeps the plan ordered by logical channel", () => {
    const entries = withChannel([{ lcn: 18, freq_hz: 451_025_000 }], {
      lcn: 17,
      freq_hz: 451_012_500,
    });

    expect(entries).toEqual([
      { lcn: 17, freq_hz: 451_012_500 },
      { lcn: 18, freq_hz: 451_025_000 },
    ]);
  });

  it("replaces a logical channel rather than listing it twice", () => {
    const entries = withChannel([{ lcn: 17, freq_hz: 451_012_500 }], {
      lcn: 17,
      freq_hz: 451_050_000,
    });

    expect(entries).toEqual([{ lcn: 17, freq_hz: 451_050_000 }]);
  });
});

describe("DMR channel search", () => {
  it("reads a range as start and end in MHz over a step in kHz", () => {
    expect(parseSearchRanges("451.0-451.5 / 12.5")).toEqual([
      { start_hz: 451_000_000, end_hz: 451_500_000, step_hz: 12_500 },
    ]);
  });

  it("refuses a range that would sweep more than the search can hold", () => {
    expect(parseSearchRanges("450-460 / 1.25")).toEqual([]);
  });

  it("refuses a step finer than the narrowest channel", () => {
    expect(parseSearchRanges("451.0-451.1 / 0.5")).toEqual([]);
  });

  it("refuses a range that ends before it starts", () => {
    expect(parseSearchRanges("451.5-451.0 / 12.5")).toEqual([]);
  });

  it("counts every frequency the range covers", () => {
    expect(searchCandidates(parseSearchRanges("451.0-451.05 / 12.5"))).toBe(5);
  });

  it("round-trips through the text the user typed", () => {
    const text = "451-451.5 / 12.5";
    expect(formatSearchRanges(parseSearchRanges(text))).toBe(text);
  });

  it("says what the search is doing right now", () => {
    expect(searchSummary([], 0, 0)).toBe("");
    expect(searchSummary(parseSearchRanges("451.0-451.05 / 12.5"), 0, 0)).toContain(
      "5 frequencies ready",
    );
    expect(searchSummary(parseSearchRanges("451.0-451.05 / 12.5"), 1, 4)).toContain(
      "Hunting 1 logical channel across 5 frequencies with 4 receivers",
    );
  });
});

describe("keeping what the search found", () => {
  const channel = (
    logical_channel: number,
    freq_hz: number,
    source: TrunkChannel["source"],
  ): TrunkChannel => ({ logical_channel, freq_hz, source, confidence: 100 });

  it("offers only the channels the search worked out for itself", () => {
    const map = [
      channel(17, 451_012_500, "learned"),
      channel(18, 451_025_000, "announced"),
      channel(19, 451_037_500, "manual"),
    ];

    expect(adoptable(map, [])).toEqual([{ lcn: 17, freq_hz: 451_012_500 }]);
  });

  it("offers nothing that was already written down", () => {
    const map = [channel(17, 451_012_500, "learned")];

    expect(adoptable(map, [{ lcn: 17, freq_hz: 451_012_500 }])).toEqual([]);
  });

  it("never offers a guess", () => {
    const map = [channel(20, 451_050_000, "predicted")];

    expect(adoptable(map, [])).toEqual([]);
  });
});

describe("the channel plan table", () => {
  const channel = (
    logical_channel: number,
    freq_hz: number,
    source: TrunkChannel["source"],
  ): TrunkChannel => ({ logical_channel, freq_hz, source, confidence: 100 });

  it("shows entered channels the server has not reported back yet", () => {
    const rows = channelPlanRows([], [{ lcn: 17, freq_hz: 451_012_500 }]);

    expect(rows).toEqual([channel(17, 451_012_500, "manual")]);
  });

  it("lets what the server knows win over what was typed", () => {
    const rows = channelPlanRows(
      [channel(17, 451_025_000, "announced")],
      [{ lcn: 17, freq_hz: 451_012_500 }],
    );

    expect(rows).toEqual([channel(17, 451_025_000, "announced")]);
  });

  it("orders the plan by logical channel", () => {
    const rows = channelPlanRows(
      [channel(30, 451_050_000, "learned"), channel(2, 451_000_000, "learned")],
      [{ lcn: 17, freq_hz: 451_012_500 }],
    );

    expect(rows.map((row) => row.logical_channel)).toEqual([2, 17, 30]);
  });

  it("counts where every frequency came from", () => {
    const rows = [
      channel(1, 451_000_000, "announced"),
      channel(2, 451_012_500, "manual"),
      channel(3, 451_025_000, "learned"),
      channel(4, 451_037_500, "predicted"),
    ];

    expect(planSummary(rows)).toBe(
      "4 logical channels — 1 announced, 1 entered, 1 found, 1 guessed.",
    );
    expect(planSummary([])).toContain("No logical channels");
  });

  it("marks a guess as something the system will not tune", () => {
    expect(usable("predicted")).toBe(false);
    expect(usable("learned")).toBe(true);
    expect(usable("manual")).toBe(true);
    expect(usable("announced")).toBe(true);
  });

  it("drops only the channel it was asked to forget", () => {
    const entries = [
      { lcn: 17, freq_hz: 451_012_500 },
      { lcn: 18, freq_hz: 451_025_000 },
    ];

    expect(withoutChannel(entries, 17)).toEqual([{ lcn: 18, freq_hz: 451_025_000 }]);
  });
});

describe("the control channel field", () => {
  it("reads a frequency in MHz", () => {
    expect(parseControlHz("451.0125")).toBe(451_012_500);
    expect(parseControlHz(" 451 ")).toBe(451_000_000);
  });

  it("clears itself rather than guessing at nonsense", () => {
    expect(parseControlHz("")).toBeUndefined();
    expect(parseControlHz("abc")).toBeUndefined();
    expect(parseControlHz("-451")).toBeUndefined();
  });

  it("says a radio waits for it instead of failing quietly", () => {
    expect(awaitingControlChannel(true, null)).toBe(true);
    expect(awaitingControlChannel(true, undefined)).toBe(true);
    expect(awaitingControlChannel(true, 451_012_500)).toBe(false);
    expect(awaitingControlChannel(false, null)).toBe(false);
  });

  it("says a named control channel the server never opened is not running", () => {
    expect(controlChannelStalled(true, 451_012_500, 0)).toBe(true);
    expect(controlChannelStalled(true, 451_012_500, 1)).toBe(false);
    expect(controlChannelStalled(true, null, 0)).toBe(false);
    expect(controlChannelStalled(false, 451_012_500, 0)).toBe(false);
    expect(controlChannelStalled(true, 451_012_500, undefined)).toBe(false);
  });
});
