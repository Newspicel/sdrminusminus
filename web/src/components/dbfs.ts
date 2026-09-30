export const METER_FLOOR_DB = -60;
export const METER_WARN_DB = -12;
export const METER_HOT_DB = -3;

export type MeterTone = "ok" | "warn" | "danger";

export function meterUnit(peakDb: number | undefined): number {
  if (peakDb === undefined || !Number.isFinite(peakDb) || peakDb <= METER_FLOOR_DB) {
    return 0;
  }
  return Math.min(1, (peakDb - METER_FLOOR_DB) / -METER_FLOOR_DB);
}

export function meterTone(peakDb: number | undefined, clipping: boolean): MeterTone {
  if (clipping || (peakDb !== undefined && peakDb >= METER_HOT_DB)) {
    return "danger";
  }
  return peakDb !== undefined && peakDb >= METER_WARN_DB ? "warn" : "ok";
}

export const HEADROOM = {
  warn: meterUnit(METER_WARN_DB),
  hot: meterUnit(METER_HOT_DB),
};

export function formatPeak(peakDb: number | undefined): string {
  if (peakDb === undefined || !Number.isFinite(peakDb) || peakDb <= METER_FLOOR_DB) {
    return "quiet";
  }
  return `${peakDb.toFixed(1)} dBFS`;
}
