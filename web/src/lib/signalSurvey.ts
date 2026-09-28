import type { SurveyCell } from "./types";

export const SIGNAL_MIN_DBFS = -120;
export const SIGNAL_MAX_DBFS = -20;

export interface SurveyView {
  radioWired: boolean;
  positionWired: boolean;
  positioned: boolean;
  targetHz: number | null;
  levelDbfs: number | null;
  recording: boolean;
  retuned: boolean;
}

export function signalOffsetLimitHz(spanHz: number, bandwidthHz: number): number {
  if (!Number.isFinite(spanHz) || !Number.isFinite(bandwidthHz)) {
    return 0;
  }
  return Math.max(0, Math.floor((spanHz - bandwidthHz) / 2));
}

export function surveyFrequencyHz(cells: readonly SurveyCell[]): number | null {
  return cells[0]?.frequency_hz ?? null;
}

export function retunedSince(cells: readonly SurveyCell[], targetHz: number | null): boolean {
  const surveyed = surveyFrequencyHz(cells);
  return surveyed !== null && targetHz !== null && Math.round(surveyed) !== Math.round(targetHz);
}

export function canStart(view: SurveyView): boolean {
  return (
    view.radioWired &&
    view.positionWired &&
    view.positioned &&
    view.levelDbfs !== null &&
    !view.retuned
  );
}

export function surveyStatus(view: SurveyView): string {
  if (!view.radioWired) {
    return "Wire a radio";
  }
  if (!view.positionWired) {
    return "Wire a position";
  }
  if (view.recording) {
    return "Recording each new GPS fix";
  }
  if (view.targetHz === null) {
    return "Waiting for the radio";
  }
  if (view.retuned) {
    return "Retuned: clear to start again";
  }
  if (view.levelDbfs === null) {
    return "Offset is outside the IQ span";
  }
  return view.positioned ? "Ready" : "Waiting for GPS";
}

export function signalSurveyCsv(
  cells: readonly SurveyCell[],
  offsetHz: number,
  bandwidthHz: number,
): string {
  const rows = cells.map((cell) =>
    [
      cell.measured_at,
      cell.frequency_hz,
      offsetHz,
      bandwidthHz,
      cell.latitude.toFixed(7),
      cell.longitude.toFixed(7),
      cell.accuracy_m?.toFixed(1) ?? "",
      cell.level_dbfs.toFixed(2),
      cell.observations,
    ].join(","),
  );
  return [
    "time,frequency_hz,offset_hz,bandwidth_hz,latitude,longitude,accuracy_m,level_dbfs,observations",
    ...rows,
  ].join("\n");
}
