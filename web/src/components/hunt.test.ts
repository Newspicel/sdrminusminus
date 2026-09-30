import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { HuntSweep, sweepNeedles } from "../canvas/nodes/HuntSweep";
import { HuntFace } from "../canvas/nodes/SinkFaces";
import type { ChannelInfo, DeviceSet, HuntStatus, PatchGraph } from "../lib/types";
import { catalogBody } from "../test/catalog";
import { renderFace } from "../test/faceHarness";
import { placed } from "../test/fixtures";
import {
  coveredLabel,
  degreesLabel,
  formatHuntDb,
  formatStrength,
  huntedHz,
  huntRefusal,
  huntSettings,
  liveHunt,
  SCANNING,
  SWEEP_TEXT,
  sweepOn,
  TUNED_AWAY,
  trend,
} from "./hunt";

const HUNT: HuntStatus = {
  settings: { channel: 9, interval_ms: 50 },
  freq_hz: 433_920_000,
  bw_hz: 12_500,
  level_db: -60,
  smooth_db: -61,
  floor_db: -90,
  best_db: -40,
  strength: 0.5,
  closing: false,
  readings: 10,
};

function deviceSet(over: Partial<DeviceSet> = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "virtual", key: "band", label: "Test band" },
    capabilities: {
      freq_ranges: [{ min: 1e6, max: 6e9 }],
      sample_rates: [2_048_000],
      gains: [],
      antennas: [],
      bandwidths: [],
      rx_streams: 1,
      tx_streams: 0,
      duplex: "rx_only",
    },
    settings: {},
    status: "running",
    channels: [],
    ...over,
  } as unknown as DeviceSet;
}

describe("liveHunt", () => {
  it("prefers the pushed reading over the one the state snapshot carried", () => {
    const set = deviceSet({ hunts: [HUNT] });
    expect(liveHunt(set, 9, undefined)).toBe(HUNT);
    const fresher = { ...HUNT, readings: 99 };
    expect(liveHunt(set, 9, fresher)?.readings).toBe(99);
  });

  it("reports nothing when the decoder is not hunted", () => {
    expect(liveHunt(deviceSet({ hunts: [HUNT] }), 3, HUNT)).toBeNull();
    expect(liveHunt(deviceSet(), 9, HUNT)).toBeNull();
    expect(liveHunt(null, 9, HUNT)).toBeNull();
  });
});

const CHANNEL: ChannelInfo = {
  id: 9,
  stream: 0,
  settings: { frequency_hz: 433_920_000, params: { type: "nfm", settings: {} } as never },
};

describe("huntRefusal", () => {
  it("names why a hunt cannot start in a word and explains it in the title", () => {
    expect(huntRefusal(null)).toBeNull();
    expect(huntRefusal({ set: deviceSet(), channel: CHANNEL })).toBeNull();
    expect(huntRefusal({ set: deviceSet(), channel: { ...CHANNEL, out_of_band: true } })).toBe(
      TUNED_AWAY,
    );
    const scan = {
      state: "scanning",
      settings: { channel: CHANNEL.id } as never,
      targets: 1,
      first_hz: 1,
      last_hz: 1,
      current_hz: 1,
      sweeps: 0,
      hits: 0,
    } as never;
    const scanning = deviceSet({ scanners: [scan] });
    expect(huntRefusal({ set: scanning, channel: CHANNEL })).toBe(SCANNING);
    expect(huntRefusal({ set: scanning, channel: { ...CHANNEL, id: 2 } })).toBeNull();
    expect(SCANNING.label).toBe("Scanning");
    expect(TUNED_AWAY.label).toBe("Tuned away");
    for (const refusal of [SCANNING, TUNED_AWAY]) {
      expect(refusal.label.split(" ").length).toBeLessThanOrEqual(2);
      expect(refusal.title.length).toBeGreaterThan(refusal.label.length);
    }
  });
});

describe("huntSettings", () => {
  it("names the hunt node so sweep and mark bearings carry it", () => {
    const sweep = {
      beamwidth_deg: 60,
      front_back_db: 15,
      min_span_deg: 180,
      min_contrast_db: 6,
      mount_offset_deg: -90,
    };
    expect(huntSettings(9, "hunt-1", sweep)).toEqual({
      channel: 9,
      interval_ms: 50,
      node: "hunt-1",
      sweep,
    });
  });
});

describe("huntedHz", () => {
  it("shows the decoder's frequency until a reading says otherwise", () => {
    expect(huntedHz(null, null)).toBeNull();
    expect(huntedHz(null, CHANNEL)).toBe(433_920_000);
    expect(huntedHz({ ...HUNT, freq_hz: 145_500_000 }, CHANNEL)).toBe(145_500_000);
    expect(huntedHz({ ...HUNT, freq_hz: 0 }, CHANNEL)).toBe(433_920_000);
  });
});

describe("trend", () => {
  it("waits for enough readings before calling a trend", () => {
    expect(trend(null)).toBe("waiting");
    expect(trend({ ...HUNT, readings: 1 })).toBe("waiting");
    expect(trend({ ...HUNT, smooth_db: null })).toBe("waiting");
  });

  it("calls warmer, colder and on top of it", () => {
    expect(trend({ ...HUNT, closing: true })).toBe("closing");
    expect(trend({ ...HUNT, closing: false, strength: 0.3 })).toBe("leaving");
    expect(trend({ ...HUNT, closing: false, strength: 0.95 })).toBe("steady");
  });
});

describe("sweep", () => {
  const SWEEP = { bins: [], covered_deg: 269.6, state: "sweeping" as const };

  it("is on in every state but off", () => {
    expect(sweepOn(null)).toBe(false);
    expect(sweepOn(undefined)).toBe(false);
    expect(sweepOn({ ...SWEEP, state: "off" })).toBe(false);
    expect(sweepOn({ bins: [], covered_deg: 0 })).toBe(false);
    expect(sweepOn(SWEEP)).toBe(true);
    expect(sweepOn({ ...SWEEP, state: "no_heading" })).toBe(true);
  });

  it("reads headings as three digits and the covered arc in whole degrees", () => {
    expect(degreesLabel(null)).toBe("-");
    expect(degreesLabel(Number.NaN)).toBe("-");
    expect(degreesLabel(7.4)).toBe("007°");
    expect(degreesLabel(359.6)).toBe("000°");
    expect(degreesLabel(-90)).toBe("270°");
    expect(coveredLabel(SWEEP)).toBe("270°");
    expect(coveredLabel({ ...SWEEP, covered_deg: -3 })).toBe("0°");
  });

  it("keeps every sweep state short", () => {
    for (const text of Object.values(SWEEP_TEXT)) {
      expect(text.label.split(" ").length).toBeLessThanOrEqual(2);
      expect(text.title).not.toMatch(/\u2014/);
    }
    expect(SWEEP_TEXT.no_heading.title).toBe("Wire a phone GPS");
  });
});

describe("formatting", () => {
  it("shows a dash rather than a number nobody measured", () => {
    expect(formatStrength(null)).toBe("-");
    expect(formatStrength({ ...HUNT, readings: 0 })).toBe("-");
    expect(formatStrength(HUNT)).toBe("50%");
    expect(formatHuntDb(null)).toBe("-");
    expect(formatHuntDb(Number.NaN)).toBe("-");
    expect(formatHuntDb(-61.25)).toBe("-61.3 dB");
  });
});

function noop(): void {}

function face(sweep: Parameters<typeof HuntSweep>[0]["sweep"], busy = false): string {
  return renderToStaticMarkup(
    createElement(HuntSweep, {
      sweep,
      busy,
      onSweep: noop,
      onEnd: noop,
      onMark: noop,
    }),
  );
}

describe("HuntSweep", () => {
  it("points the rose at the peak and the live heading", () => {
    expect(sweepNeedles(null)).toEqual([]);
    expect(sweepNeedles({ bins: [], covered_deg: 90, peak_deg: 211.7, heading_deg: 45 })).toEqual([
      { deg: 211.7, weight: "primary" },
      { deg: 45, weight: "secondary" },
    ]);
  });

  it("says None in red when the fix has no heading", () => {
    const html = face({ bins: [], covered_deg: 0, state: "no_heading" });
    expect(html).toContain('title="Wire a phone GPS"');
    expect(html).toContain("text-danger");
    expect(html).toContain(">None<");
    expect(html).toContain(">Sweep<");
    expect(html).toContain("Send your heading as a bearing");
  });

  it("offers End sweep while sweeping and reads the covered arc", () => {
    const html = face({
      bins: Array.from({ length: 72 }, (_, bin) => (bin === 42 ? 255 : 30)),
      covered_deg: 270,
      heading_deg: 91,
      peak_deg: 211.7,
      state: "sweeping",
    });
    expect(html).toContain("End sweep");
    expect(html).toContain("091°");
    expect(html).toContain("212°");
    expect(html).toContain("270°");
    expect(html).not.toContain(">None<");
  });

  it("holds both buttons while a request is out", () => {
    const html = face(null, true);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Sweep<\/button>/);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Mark<\/button>/);
    expect(face(null)).not.toContain('disabled=""');
  });
});

describe("HuntFace", () => {
  const walk = placed("walk", catalogBody("hunt"));
  const phone = placed("phone", catalogBody("gps"));

  function graph(wired: boolean): PatchGraph {
    return {
      nodes: [phone, walk],
      edges: wired
        ? [{ from: { node: "phone", port: "position" }, to: { node: "walk", port: "position" } }]
        : [],
    };
  }

  it("offers a sweep and its settings only with a position wired", () => {
    const bare = renderFace(HuntFace, walk, { graph: graph(false) });
    expect(bare).toContain("Start hunt");
    expect(bare).not.toContain(">Sweep<");
    expect(bare).not.toContain("Beamwidth");
    const wired = renderFace(HuntFace, walk, { graph: graph(true) });
    expect(wired).toContain("Turn slowly all the way round");
    expect(wired).toContain("Beamwidth");
  });
});
