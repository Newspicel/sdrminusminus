// The panel PATCHes full ChannelSettings; widening a partial edit must not lose unrelated
// fields, or the optimistic cache and the applied server state drift.
import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, ChannelSettings } from "../lib/types";
import {
  channelDecoderKind,
  channelHasAudio,
  clampOffsetHz,
  mergeChannelSettings,
  offsetLimitHz,
  rateMismatch,
} from "./channelSettings";

const base: ChannelSettings = {
  offset_hz: 25_000,
  squelch_db: -70,
  params: { type: "ssb", settings: { sideband: "lsb", bandwidth_hz: 2_400, agc: false } },
};

function descriptor(over: Partial<ChannelDescriptor>): ChannelDescriptor {
  return {
    type_id: "nfm",
    name: "NFM",
    bandwidth_hz: 12_500,
    input_rate_hz: 48_000,
    ...over,
  };
}

describe("mergeChannelSettings", () => {
  it("widens an offset edit and keeps squelch + params", () => {
    expect(mergeChannelSettings(base, { offset_hz: -12_500 })).toEqual({
      ...base,
      offset_hz: -12_500,
    });
  });

  it("distinguishes squelch off (null) from unchanged (undefined)", () => {
    expect(mergeChannelSettings(base, { squelch_db: null }).squelch_db).toBeNull();
    expect(mergeChannelSettings(base, {}).squelch_db).toBe(-70);
  });

  it("swaps params wholesale without touching placement", () => {
    const next = mergeChannelSettings(base, {
      params: { type: "nfm", settings: { bandwidth_hz: 25_000 } },
    });
    expect(next.offset_hz).toBe(25_000);
    expect(next.squelch_db).toBe(-70);
    expect(next.params).toEqual({ type: "nfm", settings: { bandwidth_hz: 25_000 } });
  });

  it("fills server defaults for absent optional fields", () => {
    const sparse: ChannelSettings = { params: { type: "nfm", settings: {} } };
    const next = mergeChannelSettings(sparse, {});
    expect(next.offset_hz).toBe(0);
    expect(next.squelch_db).toBeNull();
  });

  it("carries a decoder's edited params into the patch body", () => {
    const rtty: ChannelSettings = {
      offset_hz: 1_000,
      params: { type: "rtty", settings: { baud: 45.45, shift_hz: 170 } },
    };
    const next = mergeChannelSettings(rtty, {
      params: {
        type: "rtty",
        settings: { baud: 45.45, shift_hz: 850, stop_bits: "two", invert: true },
      },
    });
    expect(next).toEqual({
      offset_hz: 1_000,
      squelch_db: null,
      params: {
        type: "rtty",
        settings: { baud: 45.45, shift_hz: 850, stop_bits: "two", invert: true },
      },
    });
  });

  it("keeps an explicit null (auto) inside params", () => {
    const next = mergeChannelSettings(base, {
      params: { type: "morse", settings: { bandwidth_hz: 400, wpm: null } },
    });
    expect(next.params).toEqual({ type: "morse", settings: { bandwidth_hz: 400, wpm: null } });
  });
});

describe("channelHasAudio", () => {
  it("suppresses audio controls for a data decoder", () => {
    expect(
      channelHasAudio(descriptor({ type_id: "adsb", has_audio: false, decoder_kind: "adsb" })),
    ).toBe(false);
  });

  it("keeps audio for a decoder that also demodulates sound", () => {
    expect(
      channelHasAudio(descriptor({ type_id: "wfm", has_audio: true, decoder_kind: "rds" })),
    ).toBe(true);
  });

  it("assumes audio when the descriptor is unknown or predates the flag", () => {
    expect(channelHasAudio(undefined)).toBe(true);
    expect(channelHasAudio(descriptor({}))).toBe(true);
  });
});

describe("channelDecoderKind", () => {
  it("reports the emitted event kind, null for a plain demod", () => {
    expect(channelDecoderKind(descriptor({ decoder_kind: "pocsag" }))).toBe("pocsag");
    expect(channelDecoderKind(descriptor({}))).toBeNull();
    expect(channelDecoderKind(undefined)).toBeNull();
  });
});

describe("offsetLimitHz", () => {
  it("keeps the whole passband inside the span", () => {
    expect(offsetLimitHz(2_400_000, descriptor({ bandwidth_hz: 12_500 }))).toBe(1_193_750);
  });

  it("collapses to zero rather than negative when the channel fills the span", () => {
    expect(offsetLimitHz(100_000, descriptor({ bandwidth_hz: 150_000 }))).toBe(0);
  });

  it("leaves the field unbounded when the rate is unknown", () => {
    expect(offsetLimitHz(null, descriptor({}))).toBeNull();
    expect(offsetLimitHz(0, descriptor({}))).toBeNull();
  });

  it("falls back to a point channel when the type is unknown", () => {
    expect(offsetLimitHz(2_000_000, undefined)).toBe(1_000_000);
  });
});

describe("clampOffsetHz", () => {
  it("stops a step at the edge of the span, either way", () => {
    expect(clampOffsetHz(1_200_000, 1_193_750)).toBe(1_193_750);
    expect(clampOffsetHz(-1_200_000, 1_193_750)).toBe(-1_193_750);
    expect(clampOffsetHz(-25_000, 1_193_750)).toBe(-25_000);
  });

  it("leaves the offset alone while the span is unknown", () => {
    expect(clampOffsetHz(9_000_000, null)).toBe(9_000_000);
  });
});

describe("rateMismatch", () => {
  const adsb = descriptor({
    type_id: "adsb",
    name: "ADS-B",
    input_rate_hz: 2_000_000,
    native_rate_max_hz: 4_000_000,
  });
  // The rule ADS-B used to be under, kept because the descriptor can still express it.
  const fixed = descriptor({ type_id: "x", input_rate_hz: 2_000_000, exact_rate_only: true });

  it("names the range a native-rate mode runs over when the radio is outside it", () => {
    expect(rateMismatch(adsb, 1_920_000)).toEqual({ min: 2_000_000, max: 4_000_000 });
    expect(rateMismatch(adsb, 10_000_000)).toEqual({ min: 2_000_000, max: 4_000_000 });
  });

  it("is silent anywhere inside the range — 2.048 is what an RTL-SDR offers", () => {
    expect(rateMismatch(adsb, 2_048_000)).toBeNull();
    expect(rateMismatch(adsb, 2_000_000)).toBeNull();
    expect(rateMismatch(adsb, 4_000_000)).toBeNull();
  });

  it("collapses to one rate for a mode that fills its channel", () => {
    expect(rateMismatch(fixed, 2_400_000)).toEqual({ min: 2_000_000, max: 2_000_000 });
    expect(rateMismatch(fixed, 2_000_000)).toBeNull();
  });

  it("is silent for a resampling mode, and while the rate is unreported", () => {
    expect(rateMismatch(descriptor({ input_rate_hz: 48_000 }), 2_400_000)).toBeNull();
    expect(rateMismatch(adsb, null)).toBeNull();
    expect(rateMismatch(undefined, 2_400_000)).toBeNull();
  });
});
