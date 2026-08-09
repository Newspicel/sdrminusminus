// The panel PATCHes full ChannelSettings; widening a partial edit must not lose unrelated
// fields, or the optimistic cache and the applied server state drift.
import { describe, expect, it } from "vitest";
import type { ChannelSettings } from "../lib/types";
import { defaultChannelSettings, mergeChannelSettings } from "./channelSettings";

const base: ChannelSettings = {
  offset_hz: 25_000,
  squelch_db: -70,
  params: { type: "ssb", settings: { sideband: "lsb", bandwidth_hz: 2_400, agc: false } },
};

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
});

describe("defaultChannelSettings", () => {
  it("builds an empty tagged settings object per known type", () => {
    for (const typeId of ["nfm", "am", "ssb", "wfm"] as const) {
      expect(defaultChannelSettings(typeId)).toEqual({
        offset_hz: 0,
        params: { type: typeId, settings: {} },
      });
    }
  });

  it("rejects unknown type ids", () => {
    expect(defaultChannelSettings("adsb")).toBeNull();
  });
});
