import { describe, expect, it } from "vitest";
import { MAX_EXTENT_M, MIN_ELEMENTS } from "../canvas/nodes/arrayGeometry";
import { calSourceOf } from "../canvas/nodes/arrayNode";
import { BIN_OPTIONS as CORRELATOR_BINS, channelOptions } from "../canvas/nodes/correlator";
import { PEAK_OPTIONS, sourceOptions } from "../canvas/nodes/df";
import {
  cfarKindOf,
  cleaningOf,
  illuminatorOf,
  PFA_OPTIONS,
  readingAxes,
} from "../canvas/nodes/radar";
import { BIN_OPTIONS, columnOptions, STEP_OPTIONS } from "../canvas/nodes/spatialSpectrum";
import { DECAY_OPTIONS, decayWith, halfLifeTitle } from "../canvas/nodes/triangulation";
import {
  ARRAY_LIMITS,
  BAND_SEED_HZ,
  BEAMFORMER_LIMITS,
  CORRELATOR_LIMITS,
  DF_LIMITS,
  FUSION_LIMITS,
  HUNT_LIMITS,
  holds,
  LIGHT_SPEED_M_S,
  lowest,
  POLARIMETER_LIMITS,
  powersOfTwo,
  RADAR_LIMITS,
  SPATIAL_LIMITS,
  STITCH_LIMITS,
  scaled,
} from "./limits";

function values(options: readonly { value: number }[]): number[] {
  return options.map((option) => option.value);
}

describe("generated limits", () => {
  it("steps past an open lower bound only", () => {
    expect(lowest({ min: 0, max: 1, above: true }, 0.001)).toBe(0.001);
    expect(lowest({ min: 2, max: 16 }, 1)).toBe(2);
    expect(scaled(RADAR_LIMITS.overlap, 100)).toEqual({ min: 0, max: 75 });
    expect(scaled(RADAR_LIMITS.os_rank, 100)).toEqual({ min: 50, max: 95 });
    expect(holds(RADAR_LIMITS.jerk, 0)).toBe(false);
    expect(holds(RADAR_LIMITS.jerk, RADAR_LIMITS.jerk.max)).toBe(true);
    expect(holds(RADAR_LIMITS.cpi_ms, RADAR_LIMITS.cpi_ms.max + 1)).toBe(false);
  });

  it("carries the wire constants", () => {
    expect(LIGHT_SPEED_M_S).toBe(299_792_458);
    expect(RADAR_LIMITS.surface_db).toEqual({ min: -3, max: 30 });
    expect(ARRAY_LIMITS.gain_db).toEqual({ min: -20, max: 80 });
    expect(MIN_ELEMENTS).toBe(ARRAY_LIMITS.lanes.min);
    expect(MAX_EXTENT_M).toBe(ARRAY_LIMITS.extent_m);
    expect(STITCH_LIMITS.report_ms).toBeGreaterThan(0);
  });

  it("seeds every variant inside its bounds", () => {
    const seed = RADAR_LIMITS.seed;
    expect(illuminatorOf("dvbt_partial", { kind: "fm" })).toEqual({
      kind: "dvbt_partial",
      bandwidth_hz: seed.dvbt_bandwidth_hz,
    });
    expect(illuminatorOf("custom", { kind: "dab" })).toEqual({
      kind: "custom",
      bandwidth_hz: seed.custom_bandwidth_hz,
    });
    expect(cleaningOf("cma", { kind: "off" })).toEqual({
      kind: "cma",
      taps: seed.cma_taps,
      step: seed.cma_step,
    });
    expect(cfarKindOf("os", { kind: "ca" })).toEqual({ kind: "os", rank: seed.os_rank });
    expect(holds(RADAR_LIMITS.bandwidth_hz, seed.dvbt_bandwidth_hz)).toBe(true);
    expect(holds(RADAR_LIMITS.bandwidth_hz, seed.custom_bandwidth_hz)).toBe(true);
    expect(holds(RADAR_LIMITS.cma_taps, seed.cma_taps)).toBe(true);
    expect(holds(RADAR_LIMITS.cma_step, seed.cma_step)).toBe(true);
    expect(holds(RADAR_LIMITS.os_rank, seed.os_rank)).toBe(true);
    expect(calSourceOf("pilot", { kind: "noise" })).toMatchObject({
      bandwidth_hz: ARRAY_LIMITS.cal_bandwidth_seed_hz,
    });
    expect(holds(ARRAY_LIMITS.cal_bandwidth_hz, ARRAY_LIMITS.cal_bandwidth_seed_hz)).toBe(true);
    expect(decayWith("half_life", { kind: "auto" })).toEqual({
      kind: "half_life",
      seconds: FUSION_LIMITS.half_life_seed_s,
    });
    expect(holds(FUSION_LIMITS.half_life_s, FUSION_LIMITS.half_life_seed_s)).toBe(true);
  });

  it("offers only choices the server takes", () => {
    for (const option of PFA_OPTIONS) {
      expect(holds(RADAR_LIMITS.pfa, option.value)).toBe(true);
    }
    const axes = readingAxes({
      sample_rate_hz: 1,
      carrier_hz: 1,
      range_step_m: 1,
      gates: 1,
      doppler_step_hz: 1,
      doppler_rows: 1,
      batches: 1,
      cpi_ms: 1,
      hop_ms: 1,
      lanes: 1,
    });
    expect([axes.dbMin, axes.dbMax]).toEqual([
      RADAR_LIMITS.surface_db.min,
      RADAR_LIMITS.surface_db.max,
    ]);
  });

  it("names the fade half lives from the wire", () => {
    expect(halfLifeTitle(FUSION_LIMITS.fixed_half_life_s)).toBe("Half life 1 min");
    expect(halfLifeTitle(FUSION_LIMITS.moving_half_life_s)).toBe("Half life 30 min");
    expect(halfLifeTitle(45)).toBe("Half life 45 s");
    expect(DECAY_OPTIONS.map((option) => option.title)).toContain("Half life 30 min");
  });

  it("lists the powers of two a bound holds", () => {
    expect(powersOfTwo({ min: 64, max: 512 })).toEqual([64, 128, 256, 512]);
    expect(powersOfTwo({ min: 100, max: 100 })).toEqual([]);
  });

  it("offers the array processors only what the server takes", () => {
    expect(values(BIN_OPTIONS)).toEqual([256, 512, 1024, 2048, 4096]);
    expect(values(columnOptions(4096))).toEqual([64, 128, 256, 512, 1024]);
    expect(values(STEP_OPTIONS)).toEqual([1, 2, 3, 4, 5, 6, 8, 9, 10]);
    expect(values(CORRELATOR_BINS)).toEqual(powersOfTwo(CORRELATOR_LIMITS.bins));
    expect(values(CORRELATOR_BINS)[0]).toBe(64);
    expect(values(channelOptions(8192))).toEqual([16, 32, 64, 128, 256, 512, 1024]);
    expect(values(PEAK_OPTIONS)).toEqual([1, 2, 3, 4]);
    expect(sourceOptions(64)).toHaveLength(DF_LIMITS.sources.max + 1);
    for (const deg of values(STEP_OPTIONS)) {
      expect(holds(SPATIAL_LIMITS.azimuth_step_deg, deg)).toBe(true);
    }
  });

  it("carries the processor and hunt ranges", () => {
    expect(DF_LIMITS.report_ms).toEqual({ min: 100, max: 10_000 });
    expect(scaled(DF_LIMITS.band.bandwidth_hz, 1e-3)).toEqual({ min: 0.1, max: 20_000 });
    expect(DF_LIMITS.station_len).toBe(64);
    expect(BEAMFORMER_LIMITS.nulls).toBe(3);
    expect(BEAMFORMER_LIMITS.band.bandwidth_hz.min).toBe(1_000);
    expect(POLARIMETER_LIMITS.band.bandwidth_hz.min).toBe(100);
    expect(SPATIAL_LIMITS.report_ms).toEqual({ min: 50, max: 2_000 });
    expect(CORRELATOR_LIMITS.integrate_s).toEqual({ min: 0.05, max: 600 });
    expect(HUNT_LIMITS.mount_offset_deg).toEqual({ min: -180, max: 180 });
    for (const band of [BEAMFORMER_LIMITS.band, SPATIAL_LIMITS.band, CORRELATOR_LIMITS.band]) {
      expect(holds(band.bandwidth_hz, BAND_SEED_HZ)).toBe(true);
    }
  });
});
