// The panel PATCHes full ChannelSettings; widening a partial edit must not lose unrelated
// fields, or the optimistic cache and the applied server state drift.
import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, ChannelSettings } from "../lib/types";
import {
  type ChannelTypeId,
  channelDecoderKind,
  channelHasAudio,
  defaultChannelSettings,
  isChannelTypeId,
  mergeChannelSettings,
} from "./channelSettings";

const base: ChannelSettings = {
  offset_hz: 25_000,
  squelch_db: -70,
  params: { type: "ssb", settings: { sideband: "lsb", bandwidth_hz: 2_400, agc: false } },
};

const TYPE_IDS: ChannelTypeId[] = [
  "nfm",
  "am",
  "ssb",
  "wfm",
  "pocsag",
  "adsb",
  "ais",
  "aprs",
  "rtty",
  "morse",
];

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

describe("defaultChannelSettings", () => {
  it("builds an empty tagged settings object per known type", () => {
    for (const typeId of TYPE_IDS) {
      expect(defaultChannelSettings(typeId)).toEqual({
        offset_hz: 0,
        params: { type: typeId, settings: {} },
      });
    }
  });

  it("hands out a fresh object so an edit cannot poison the next add", () => {
    const first = defaultChannelSettings("adsb");
    const second = defaultChannelSettings("adsb");
    expect(first).not.toBe(second);
    expect(first?.params.settings).not.toBe(second?.params.settings);
  });

  it("rejects unknown type ids", () => {
    expect(defaultChannelSettings("dmr")).toBeNull();
    expect(isChannelTypeId("dmr")).toBe(false);
    expect(isChannelTypeId("pocsag")).toBe(true);
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
