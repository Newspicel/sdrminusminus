import limits from "../generated/limits.json";

export interface Bounds {
  min: number;
  max: number;
  above?: boolean;
}

export interface BandLimits {
  offset_hz: Bounds;
  bandwidth_hz: Bounds;
}

export const LIGHT_SPEED_M_S = limits.light_speed_m_s;
export const BAND_SEED_HZ = limits.band_seed_hz;
export const ARRAY_LIMITS = limits.array;
export const FUSION_LIMITS = limits.fusion;
export const RADAR_LIMITS = limits.radar;
export const STITCH_LIMITS = limits.stitch;
export const DF_LIMITS = limits.df;
export const BEAMFORMER_LIMITS = limits.beamformer;
export const SPATIAL_LIMITS = limits.spatial;
export const CORRELATOR_LIMITS = limits.correlator;
export const POLARIMETER_LIMITS = limits.polarimeter;
export const HUNT_LIMITS = limits.hunt;

export function lowest(bounds: Bounds, step: number): number {
  return bounds.above === true ? bounds.min + step : bounds.min;
}

export function scaled(bounds: Bounds, factor: number): Bounds {
  return { ...bounds, min: bounds.min * factor, max: bounds.max * factor };
}

export function holds(bounds: Bounds, value: number): boolean {
  const low = bounds.above === true ? value > bounds.min : value >= bounds.min;
  return low && value <= bounds.max;
}

export function powersOfTwo(bounds: Bounds): number[] {
  const powers: number[] = [];
  for (let value = 1; value <= bounds.max; value *= 2) {
    if (holds(bounds, value)) {
      powers.push(value);
    }
  }
  return powers;
}
