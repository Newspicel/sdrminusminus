import { describe, expect, it } from "vitest";
import type { AudioProcessing, ChannelDescriptor, ChannelSettings } from "../lib/types";
import {
  AUDIO_LIMITS,
  audioChainActive,
  channelDecoderKind,
  channelHasAudio,
  channelWidthHz,
  clampOffsetHz,
  limitOf,
  mergeAudio,
  mergeChannelSettings,
  nudgedSquelch,
  offsetForFrequencyHz,
  offsetLimitHz,
  radioWindowHz,
  reachesHz,
  scaledLimit,
  squelchAt,
  squelchLevelDb,
  squelchMarginDb,
  squelchMode,
  withNotchAdded,
  withNotchAt,
  withNotchRemoved,
} from "./channelSettings";

const base: ChannelSettings = {
  frequency_hz: 145_025_000,
  squelch: { mode: "manual", level_db: -70 },
  params: { type: "ssb", settings: { sideband: "lsb", bandwidth_hz: 2_400 } },
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
  it("widens a frequency edit and keeps squelch + params", () => {
    expect(mergeChannelSettings(base, { frequency_hz: 144_987_500 })).toEqual({
      ...base,
      blanker: {},
      frequency_hz: 144_987_500,
    });
  });

  it("carries the blanker through an unrelated edit", () => {
    const blanked: ChannelSettings = { ...base, blanker: { enabled: true, threshold: 7 } };
    expect(mergeChannelSettings(blanked, { frequency_hz: 145_000_000 }).blanker).toEqual({
      enabled: true,
      threshold: 7,
    });
  });

  it("replaces the squelch whole and keeps it when the edit says nothing", () => {
    expect(mergeChannelSettings(base, { squelch: { mode: "off" } }).squelch).toEqual({
      mode: "off",
    });
    expect(mergeChannelSettings(base, {}).squelch).toEqual({ mode: "manual", level_db: -70 });
    expect(mergeChannelSettings(base, { squelch: { mode: "auto", margin_db: 8 } }).squelch).toEqual(
      { mode: "auto", margin_db: 8 },
    );
  });

  it("swaps params wholesale without touching placement", () => {
    const next = mergeChannelSettings(base, {
      params: { type: "nfm", settings: { bandwidth_hz: 25_000 } },
    });
    expect(next.frequency_hz).toBe(145_025_000);
    expect(next.squelch).toEqual({ mode: "manual", level_db: -70 });
    expect(next.params).toEqual({ type: "nfm", settings: { bandwidth_hz: 25_000 } });
  });

  it("fills server defaults for absent optional fields", () => {
    const sparse: ChannelSettings = {
      frequency_hz: 145_000_000,
      params: { type: "nfm", settings: {} },
    };
    const next = mergeChannelSettings(sparse, {});
    expect(next.frequency_hz).toBe(145_000_000);
    expect(next.squelch).toEqual({ mode: "off" });
  });

  it("carries a decoder's edited params into the patch body", () => {
    const rtty: ChannelSettings = {
      frequency_hz: 14_070_000,
      params: { type: "rtty", settings: { baud: 45.45, shift_hz: 170 } },
    };
    const next = mergeChannelSettings(rtty, {
      params: {
        type: "rtty",
        settings: { baud: 45.45, shift_hz: 850, stop_bits: "two", invert: true },
      },
    });
    expect(next).toEqual({
      frequency_hz: 14_070_000,
      squelch: { mode: "off" },
      blanker: {},
      params: {
        type: "rtty",
        settings: { baud: 45.45, shift_hz: 850, stop_bits: "two", invert: true },
      },
    });
  });

  it("keeps the DAB transmission mode and service when tuning", () => {
    for (const transmission_mode of ["i", "ii", "iii", "iv"] as const) {
      const dab: ChannelSettings = {
        frequency_hz: 220_352_000,
        params: {
          type: "dab",
          settings: { transmission_mode, mode: "dab_plus", service_id: 49569 },
        },
      };
      const next = mergeChannelSettings(dab, { frequency_hz: 225_648_000 });
      expect(next.params).toEqual(dab.params);
      expect(next.frequency_hz).toBe(225_648_000);
    }
  });

  it("keeps an explicit null (auto) inside params", () => {
    const next = mergeChannelSettings(base, {
      params: { type: "morse", settings: { bandwidth_hz: 400, wpm: null } },
    });
    expect(next.params).toEqual({ type: "morse", settings: { bandwidth_hz: 400, wpm: null } });
  });

  it("widens a pager bandwidth or inversion edit over the settings beside it", () => {
    for (const type of ["flex", "ermes"] as const) {
      const pager: ChannelSettings = {
        frequency_hz: 169_650_000,
        params: { type, settings: { bandwidth_hz: 12_500, invert: false } },
      };
      const wider = mergeChannelSettings(pager, {
        params: { type, settings: { bandwidth_hz: 25_000, invert: false } },
      });
      expect(wider.params).toEqual({
        type,
        settings: { bandwidth_hz: 25_000, invert: false },
      });
      const inverted = mergeChannelSettings(wider, {
        params: { type, settings: { bandwidth_hz: 25_000, invert: true } },
      });
      expect(inverted.params).toEqual({
        type,
        settings: { bandwidth_hz: 25_000, invert: true },
      });
    }
  });

  it("carries every CW skimmer setting, and an auto speed, into the patch body", () => {
    const skimmer: ChannelSettings = {
      frequency_hz: 14_030_000,
      params: {
        type: "cw_skimmer",
        settings: { bandwidth_hz: 24_000, threshold_db: 10, max_signals: 32, wpm: 20 },
      },
    };
    const next = mergeChannelSettings(skimmer, {
      params: {
        type: "cw_skimmer",
        settings: { bandwidth_hz: 16_000, threshold_db: 8, max_signals: 8, wpm: null },
      },
    });
    expect(next.params).toEqual({
      type: "cw_skimmer",
      settings: { bandwidth_hz: 16_000, threshold_db: 8, max_signals: 8, wpm: null },
    });
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

describe("channelWidthHz", () => {
  it("prefers the decoder's own bandwidth setting", () => {
    expect(
      channelWidthHz(
        { type: "am", settings: { bandwidth_hz: 8_000 } } as ChannelSettings["params"],
        descriptor({ bandwidth_hz: 10_000 }),
      ),
    ).toBe(8_000);
  });

  it("falls back to the channel type width", () => {
    expect(channelWidthHz(undefined, descriptor({ bandwidth_hz: 12_500 }))).toBe(12_500);
  });

  it("hides a missing or zero width", () => {
    expect(channelWidthHz(undefined, descriptor({ bandwidth_hz: 0 }))).toBeNull();
    expect(channelWidthHz(undefined, undefined)).toBeNull();
  });
});

describe("radioWindowHz", () => {
  it("names the edges a radio can hear a channel of this width between", () => {
    expect(radioWindowHz(145_000_000, 2_400_000, descriptor({ bandwidth_hz: 12_500 }))).toEqual({
      lowHz: 143_806_250,
      highHz: 146_193_750,
    });
  });

  it("is unknown while the radio's centre or rate is", () => {
    expect(radioWindowHz(null, 2_400_000, descriptor({}))).toBeNull();
    expect(radioWindowHz(145_000_000, null, descriptor({}))).toBeNull();
  });
});

describe("reachesHz", () => {
  const window = { lowHz: 143_806_250, highHz: 146_193_750 };

  it("holds inside the window and at its edges", () => {
    expect(reachesHz(145_000_000, window)).toBe(true);
    expect(reachesHz(window.lowHz, window)).toBe(true);
    expect(reachesHz(window.highHz, window)).toBe(true);
  });

  it("fails past either edge", () => {
    expect(reachesHz(143_000_000, window)).toBe(false);
    expect(reachesHz(147_000_000, window)).toBe(false);
  });

  it("assumes reach while there is no radio to judge against", () => {
    expect(reachesHz(1_090_000_000, null)).toBe(true);
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

describe("mergeAudio", () => {
  it("widens one stage's edit over the stages beside it", () => {
    const current: AudioProcessing = { agc: "medium", denoise: { enabled: true, strength: 0.4 } };
    expect(mergeAudio(current, { auto_notch: true })).toEqual({
      agc: "medium",
      denoise: { enabled: true, strength: 0.4 },
      auto_notch: true,
    });
  });

  it("starts from an empty chain", () => {
    expect(mergeAudio({}, { agc: "fast" })).toEqual({ agc: "fast" });
  });
});

describe("notch editing", () => {
  const one = { freq_hz: 1_000, width_hz: 100 };

  it("appends at the defaults until the channel is full", () => {
    const full = Array.from({ length: AUDIO_LIMITS.maxNotches }, () => one);
    expect(withNotchAdded([])).toEqual([one]);
    expect(withNotchAdded(full)).toBeNull();
  });

  it("edits and removes by position, leaving the others alone", () => {
    const notches = [one, { freq_hz: 2_000, width_hz: 50 }];
    expect(withNotchAt(notches, 1, { freq_hz: 2_500 })).toEqual([
      one,
      { freq_hz: 2_500, width_hz: 50 },
    ]);
    expect(withNotchRemoved(notches, 0)).toEqual([{ freq_hz: 2_000, width_hz: 50 }]);
  });
});

describe("audioChainActive", () => {
  it("is false for an absent or all-off chain", () => {
    expect(audioChainActive(undefined)).toBe(false);
    expect(audioChainActive({})).toBe(false);
    expect(audioChainActive({ agc: "off", notches: [], denoise: { enabled: false } })).toBe(false);
  });

  it("is true as soon as any one stage is doing something", () => {
    expect(audioChainActive({ agc: "slow" })).toBe(true);
    expect(audioChainActive({ auto_notch: true })).toBe(true);
    expect(audioChainActive({ denoise: { enabled: true } })).toBe(true);
    expect(audioChainActive({ filter: { enabled: true } })).toBe(true);
    expect(audioChainActive({ click_removal: { enabled: true } })).toBe(true);
    expect(audioChainActive({ notches: [{ freq_hz: 1_000, width_hz: 100 }] })).toBe(true);
  });
});

describe("offsetForFrequencyHz", () => {
  it("turns an absolute frequency into an offset from the tuned center", () => {
    expect(offsetForFrequencyHz(433_920_000, 433_000_000, 1_000_000)).toBe(920_000);
    expect(offsetForFrequencyHz(432_500_000, 433_000_000, 1_000_000)).toBe(-500_000);
    expect(offsetForFrequencyHz(433_000_000, 433_000_000, null)).toBe(0);
  });

  it("rounds to whole hertz", () => {
    expect(offsetForFrequencyHz(433_920_000.4, 433_000_000, null)).toBe(920_000);
  });

  it("refuses a frequency the current span cannot reach", () => {
    expect(offsetForFrequencyHz(435_000_000, 433_000_000, 1_000_000)).toBeNull();
    expect(offsetForFrequencyHz(435_000_000, 433_000_000, null)).toBe(2_000_000);
  });

  it("has no answer without finite inputs", () => {
    expect(offsetForFrequencyHz(Number.NaN, 433_000_000, null)).toBeNull();
    expect(offsetForFrequencyHz(433_000_000, Number.POSITIVE_INFINITY, null)).toBeNull();
  });
});

describe("squelchMode", () => {
  it("reads the gate state off the tagged value, off when absent", () => {
    expect(squelchMode(undefined)).toBe("off");
    expect(squelchMode({ mode: "off" })).toBe("off");
    expect(squelchMode({ mode: "manual", level_db: -60 })).toBe("manual");
    expect(squelchMode({ mode: "auto", margin_db: 8 })).toBe("auto");
    expect(squelchLevelDb({ mode: "manual", level_db: -60 })).toBe(-60);
    expect(squelchLevelDb({ mode: "auto", margin_db: 8 })).toBeNull();
    expect(squelchMarginDb({ mode: "auto", margin_db: 8 })).toBe(8);
    expect(squelchMarginDb(undefined)).toBeNull();
  });
});

describe("squelchAt", () => {
  const held = { levelDb: -55, marginDb: 12 };

  it("builds each mode from the values held for it", () => {
    expect(squelchAt("off", held)).toEqual({ mode: "off" });
    expect(squelchAt("manual", held)).toEqual({ mode: "manual", level_db: -55 });
    expect(squelchAt("auto", held)).toEqual({ mode: "auto", margin_db: 12 });
  });
});

describe("nudgedSquelch", () => {
  it("moves a manual level and clamps it to the slider range", () => {
    expect(nudgedSquelch({ mode: "manual", level_db: -60 }, 2)).toEqual({
      mode: "manual",
      level_db: -58,
    });
    expect(nudgedSquelch({ mode: "manual", level_db: -1 }, 2)).toEqual({
      mode: "manual",
      level_db: 0,
    });
  });

  it("opens a gate that was off at the default level", () => {
    expect(nudgedSquelch(undefined, -2)).toEqual({ mode: "manual", level_db: -62 });
    expect(nudgedSquelch({ mode: "off" }, 2)).toEqual({ mode: "manual", level_db: -58 });
  });

  it("moves the margin instead when the gate tracks the noise floor", () => {
    expect(nudgedSquelch({ mode: "auto", margin_db: 8 }, 2)).toEqual({
      mode: "auto",
      margin_db: 10,
    });
    expect(nudgedSquelch({ mode: "auto", margin_db: 39 }, 2)).toEqual({
      mode: "auto",
      margin_db: 40,
    });
  });
});

describe("limitOf", () => {
  const limits = [
    { name: "wpm", min: 5, max: 60, step: 1 },
    { name: "threshold", min: 1.5, max: 100 },
  ];

  it("hands a number field the range its decoder published", () => {
    expect(limitOf(limits, "wpm")).toEqual({ min: 5, max: 60, step: 1 });
    expect(limitOf(limits, "threshold")).toEqual({ min: 1.5, max: 100, step: undefined });
  });

  it("leaves a field unconstrained when nothing was published", () => {
    expect(limitOf(limits, "baud")).toEqual({});
    expect(limitOf(undefined, "wpm")).toEqual({});
  });

  it("scales a limit into the unit a field is shown in", () => {
    expect(scaledLimit({ min: 500_000, max: 9_000_000, step: 500_000 }, 1e-6)).toEqual({
      min: 0.5,
      max: 9,
      step: 0.5,
    });
    expect(scaledLimit({}, 1e-6)).toEqual({ min: undefined, max: undefined, step: undefined });
  });
});
