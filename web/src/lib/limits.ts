import limits from "../generated/limits.json";

export interface Bounds {
  min: number;
  max: number;
  above?: boolean;
}

export const LIGHT_SPEED_M_S = limits.light_speed_m_s;
export const ARRAY_LIMITS = limits.array;
export const FUSION_LIMITS = limits.fusion;
export const RADAR_LIMITS = limits.radar;
export const STITCH_LIMITS = limits.stitch;

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
