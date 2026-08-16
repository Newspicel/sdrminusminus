import { greatCircleKm, type PropagationCell } from "./propagation";
import type { IonosondeStation } from "./types";

export const FORECAST_RADIUS_KM = 3_000;

export const FORECAST_MIN_STATIONS = 2;

const NEAR_KM = 25;

export interface ForecastPoint {
  muf3000Mhz: number;
  stations: number;
  nearestKm: number;
  nearest: string;
}

export function forecastAt(
  stations: readonly IonosondeStation[],
  latitude: number,
  longitude: number,
  radiusKm = FORECAST_RADIUS_KM,
): ForecastPoint | null {
  let weighted = 0;
  let weights = 0;
  let used = 0;
  let nearestKm = Number.POSITIVE_INFINITY;
  let nearest = "";
  for (const station of stations) {
    const distanceKm = greatCircleKm([latitude, longitude], [station.latitude, station.longitude]);
    if (distanceKm < nearestKm) {
      nearestKm = distanceKm;
      nearest = station.name === "" ? station.code : station.name;
    }
    if (distanceKm > radiusKm) {
      continue;
    }
    const confidence = Math.max(0.1, (station.confidence ?? 100) / 100);
    const weight = confidence / Math.max(NEAR_KM, distanceKm) ** 2;
    weighted += weight * station.muf3000_mhz;
    weights += weight;
    used += 1;
  }
  if (used < FORECAST_MIN_STATIONS || weights <= 0) {
    return null;
  }
  return { muf3000Mhz: weighted / weights, stations: used, nearestKm, nearest };
}

export interface CellComparison {
  cell: PropagationCell;
  measuredMuf3000Mhz: number;
  forecast: ForecastPoint;
  deltaMhz: number;
}

export function compareCells(
  cells: readonly PropagationCell[],
  stations: readonly IonosondeStation[],
  radiusKm = FORECAST_RADIUS_KM,
): CellComparison[] {
  const compared: CellComparison[] = [];
  for (const cell of cells) {
    const measured = cell.measuredMuf3000Mhz;
    if (measured === null) {
      continue;
    }
    const forecast = forecastAt(stations, cell.latitude, cell.longitude, radiusKm);
    if (forecast === null) {
      continue;
    }
    compared.push({
      cell,
      measuredMuf3000Mhz: measured,
      forecast,
      deltaMhz: measured - forecast.muf3000Mhz,
    });
  }
  return compared;
}

export interface ForecastAgreement {
  cells: number;
  above: number;
  medianDeltaMhz: number;
  widestAbove: CellComparison | null;
}

export function forecastAgreement(comparisons: readonly CellComparison[]): ForecastAgreement {
  if (comparisons.length === 0) {
    return { cells: 0, above: 0, medianDeltaMhz: 0, widestAbove: null };
  }
  const deltas = comparisons.map((entry) => entry.deltaMhz).toSorted((a, b) => a - b);
  const middle = Math.floor(deltas.length / 2);
  const medianDeltaMhz =
    deltas.length % 2 === 1
      ? (deltas[middle] ?? 0)
      : ((deltas[middle - 1] ?? 0) + (deltas[middle] ?? 0)) / 2;
  let widestAbove: CellComparison | null = null;
  let above = 0;
  for (const entry of comparisons) {
    if (entry.deltaMhz <= 0) {
      continue;
    }
    above += 1;
    if (widestAbove === null || entry.deltaMhz > widestAbove.deltaMhz) {
      widestAbove = entry;
    }
  }
  return { cells: comparisons.length, above, medianDeltaMhz, widestAbove };
}
