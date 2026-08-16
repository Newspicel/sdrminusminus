import { create } from "zustand";
import type { DecodedRecord, DecoderEvent, DecoderKind, PositionFix } from "./types";

export const PROPAGATION_KINDS = ["ft8", "ft4", "wspr"] as const satisfies readonly DecoderKind[];

export type PropagationKind = (typeof PROPAGATION_KINDS)[number];

export const EARTH_RADIUS_KM = 6371;

export const MIN_MUF_PATH_KM = 500;

export const OBSERVATION_CAPACITY = 20_000;

export const DECAYS_KEPT = 8;

export const DEFAULT_HALF_LIFE_MIN = 30;

export const DEFAULT_REFLECTION_HEIGHT_KM = 300;

export const HISTORY_WINDOW_MS = 6 * 60 * 60_000;

const RR73 = "RR73";

const GRID4 = /^[A-R][A-R][0-9][0-9]$/;

export interface PathObservation {
  key: string;
  at: number;
  kind: PropagationKind;
  callsign: string;
  grid: string;
  freqHz: number;
  snrDb: number;
  latitude: number;
  longitude: number;
  distanceKm: number;
  bearingDeg: number;
  hops: number;
  muf3000Mhz: number | null;
  control: readonly (readonly [number, number])[];
}

export interface PropagationCell {
  key: string;
  latitude: number;
  longitude: number;
  weight: number;
  decodes: number;
  callsigns: number;
  bestFreqHz: number;
  bestSnrDb: number;
  measuredMuf3000Mhz: number | null;
  medianDistanceKm: number;
  lastSeen: number;
}

export interface PropagationPath {
  key: string;
  from: readonly [number, number];
  to: readonly [number, number];
  weight: number;
  freqHz: number;
}

export interface PropagationOptions {
  halfLifeMinutes: number;
  nowMs: number;
}

export function gridToLatLon(grid: string): [number, number] | null {
  const text = grid.trim().toUpperCase();
  if (text.length < 4 || text.length % 2 !== 0 || text.length > 8) {
    return null;
  }
  const at = (index: number): number => text.charCodeAt(index);
  const field = [at(0) - 65, at(1) - 65];
  if (field[0] === undefined || field[1] === undefined) {
    return null;
  }
  if (field[0] < 0 || field[0] > 17 || field[1] < 0 || field[1] > 17) {
    return null;
  }
  const square = [at(2) - 48, at(3) - 48];
  if (square[0] === undefined || square[1] === undefined) {
    return null;
  }
  if (square[0] < 0 || square[0] > 9 || square[1] < 0 || square[1] > 9) {
    return null;
  }
  let lon = field[0] * 20 + square[0] * 2;
  let lat = field[1] * 10 + square[1];
  let lonSize = 2;
  let latSize = 1;

  if (text.length >= 6) {
    const sub = [at(4) - 65, at(5) - 65];
    if (sub[0] === undefined || sub[1] === undefined) {
      return null;
    }
    if (sub[0] < 0 || sub[0] > 23 || sub[1] < 0 || sub[1] > 23) {
      return null;
    }
    lonSize /= 24;
    latSize /= 24;
    lon += sub[0] * lonSize;
    lat += sub[1] * latSize;
  }
  if (text.length === 8) {
    const ext = [at(6) - 48, at(7) - 48];
    if (ext[0] === undefined || ext[1] === undefined) {
      return null;
    }
    if (ext[0] < 0 || ext[0] > 9 || ext[1] < 0 || ext[1] > 9) {
      return null;
    }
    lonSize /= 10;
    latSize /= 10;
    lon += ext[0] * lonSize;
    lat += ext[1] * latSize;
  }
  return [lat + latSize / 2 - 90, lon + lonSize / 2 - 180];
}

export function latLonToGrid(latitude: number, longitude: number): string {
  const lon = Math.min(359.999_999, Math.max(0, longitude + 180));
  const lat = Math.min(179.999_999, Math.max(0, latitude + 90));
  const lonField = Math.floor(lon / 20);
  const latField = Math.floor(lat / 10);
  const lonSquare = Math.floor((lon % 20) / 2);
  const latSquare = Math.floor(lat % 10);
  return `${String.fromCharCode(65 + lonField)}${String.fromCharCode(65 + latField)}${lonSquare}${latSquare}`;
}

export function messageGrid(text: string): string | null {
  const tokens = text.trim().toUpperCase().split(/\s+/);
  const last = tokens.at(-1);
  if (last === undefined || last === RR73 || !GRID4.test(last)) {
    return null;
  }
  return last;
}

export function messageCallsign(text: string): string | null {
  const tokens = text.trim().toUpperCase().split(/\s+/);
  const beforeGrid = tokens.at(-2);
  if (beforeGrid === undefined || beforeGrid === "") {
    return null;
  }
  return beforeGrid.replace(/^<|>$/g, "");
}

export interface Spot {
  grid: string;
  callsign: string;
  snrDb: number;
}

export function eventSpot(event: DecoderEvent): Spot | null {
  switch (event.kind) {
    case "ft8":
    case "ft4": {
      const grid = messageGrid(event.data.text);
      if (grid === null) {
        return null;
      }
      return {
        grid,
        callsign: messageCallsign(event.data.text) ?? "",
        snrDb: event.data.snr_db,
      };
    }
    case "wspr": {
      const grid = event.data.grid ?? null;
      if (grid === null || gridToLatLon(grid) === null) {
        return null;
      }
      return { grid, callsign: event.data.callsign, snrDb: event.data.snr_db };
    }
    default:
      return null;
  }
}

export function isPropagationKind(kind: string): kind is PropagationKind {
  return (PROPAGATION_KINDS as readonly string[]).includes(kind);
}

const toRad = (deg: number): number => (deg * Math.PI) / 180;
const toDeg = (rad: number): number => (rad * 180) / Math.PI;

export function greatCircleKm(
  from: readonly [number, number],
  to: readonly [number, number],
): number {
  const [lat1, lon1] = [toRad(from[0]), toRad(from[1])];
  const [lat2, lon2] = [toRad(to[0]), toRad(to[1])];
  const sinLat = Math.sin((lat2 - lat1) / 2);
  const sinLon = Math.sin((lon2 - lon1) / 2);
  const a = sinLat * sinLat + Math.cos(lat1) * Math.cos(lat2) * sinLon * sinLon;
  return 2 * EARTH_RADIUS_KM * Math.asin(Math.min(1, Math.sqrt(a)));
}

export function bearingDeg(from: readonly [number, number], to: readonly [number, number]): number {
  const [lat1, lon1] = [toRad(from[0]), toRad(from[1])];
  const [lat2, lon2] = [toRad(to[0]), toRad(to[1])];
  const dLon = lon2 - lon1;
  const y = Math.sin(dLon) * Math.cos(lat2);
  const x = Math.cos(lat1) * Math.sin(lat2) - Math.sin(lat1) * Math.cos(lat2) * Math.cos(dLon);
  return (toDeg(Math.atan2(y, x)) + 360) % 360;
}

export function alongGreatCircle(
  from: readonly [number, number],
  to: readonly [number, number],
  fraction: number,
): [number, number] {
  const [lat1, lon1] = [toRad(from[0]), toRad(from[1])];
  const [lat2, lon2] = [toRad(to[0]), toRad(to[1])];
  const d = greatCircleKm(from, to) / EARTH_RADIUS_KM;
  if (d < 1e-9) {
    return [from[0], from[1]];
  }
  const a = Math.sin((1 - fraction) * d) / Math.sin(d);
  const b = Math.sin(fraction * d) / Math.sin(d);
  const x = a * Math.cos(lat1) * Math.cos(lon1) + b * Math.cos(lat2) * Math.cos(lon2);
  const y = a * Math.cos(lat1) * Math.sin(lon1) + b * Math.cos(lat2) * Math.sin(lon2);
  const z = a * Math.sin(lat1) + b * Math.sin(lat2);
  return [toDeg(Math.atan2(z, Math.hypot(x, y))), toDeg(Math.atan2(y, x))];
}

export function maxHopKm(heightKm: number): number {
  const r = EARTH_RADIUS_KM;
  return 2 * r * Math.acos(Math.min(1, r / (r + heightKm)));
}

export function hopCount(distanceKm: number, heightKm: number): number {
  const reach = maxHopKm(heightKm);
  if (!(reach > 0) || !(distanceKm > 0)) {
    return 1;
  }
  return Math.max(1, Math.ceil(distanceKm / reach));
}

export function obliquityFactor(hopKm: number, heightKm: number): number {
  const r = EARTH_RADIUS_KM;
  const delta = hopKm / (2 * r);
  const denominator = r + heightKm - r * Math.cos(delta);
  if (!(denominator > 0)) {
    return 1;
  }
  return Math.hypot(1, (r * Math.sin(delta)) / denominator);
}

export function muf3000Mhz(freqHz: number, distanceKm: number, heightKm: number): number | null {
  if (!Number.isFinite(freqHz) || freqHz <= 0 || distanceKm < MIN_MUF_PATH_KM) {
    return null;
  }
  const hops = hopCount(distanceKm, heightKm);
  const factor = obliquityFactor(distanceKm / hops, heightKm);
  if (!(factor > 0)) {
    return null;
  }
  return ((freqHz / 1e6) * obliquityFactor(3000, heightKm)) / factor;
}

export function controlPoints(
  from: readonly [number, number],
  to: readonly [number, number],
  hops: number,
): [number, number][] {
  const points: [number, number][] = [];
  for (let hop = 0; hop < hops; hop += 1) {
    points.push(alongGreatCircle(from, to, (2 * hop + 1) / (2 * hops)));
  }
  return points;
}

export function observationOf(
  record: DecodedRecord,
  receiver: readonly [number, number],
  heightKm: number,
): PathObservation | null {
  if (!isPropagationKind(record.event.kind)) {
    return null;
  }
  const spot = eventSpot(record.event);
  if (spot === null) {
    return null;
  }
  const transmitter = gridToLatLon(spot.grid);
  if (transmitter === null) {
    return null;
  }
  const at = Date.parse(record.at);
  const distanceKm = greatCircleKm(receiver, transmitter);
  const hops = hopCount(distanceKm, heightKm);
  return {
    key: `${record.at}|${record.device_set}:${record.channel}|${spot.callsign}|${spot.grid}`,
    at: Number.isNaN(at) ? Date.now() : at,
    kind: record.event.kind,
    callsign: spot.callsign,
    grid: spot.grid,
    freqHz: record.freq_hz,
    snrDb: spot.snrDb,
    latitude: transmitter[0],
    longitude: transmitter[1],
    distanceKm,
    bearingDeg: bearingDeg(receiver, transmitter),
    hops,
    muf3000Mhz: muf3000Mhz(record.freq_hz, distanceKm, heightKm),
    control: controlPoints(receiver, transmitter, hops),
  };
}

export function decayWeight(ageMs: number, halfLifeMinutes: number): number {
  const halfLifeMs = Math.max(1, halfLifeMinutes) * 60_000;
  if (ageMs <= 0) {
    return 1;
  }
  return 2 ** (-ageMs / halfLifeMs);
}

export function propagationCells(
  observations: readonly PathObservation[],
  options: PropagationOptions,
): PropagationCell[] {
  const cells = new Map<string, PropagationCell & { calls: Set<string>; distances: number[] }>();
  for (const observation of observations) {
    const weight = decayWeight(options.nowMs - observation.at, options.halfLifeMinutes);
    if (weight <= 0) {
      continue;
    }
    for (const [latitude, longitude] of observation.control) {
      const key = latLonToGrid(latitude, longitude);
      const centre = gridToLatLon(key);
      if (centre === null) {
        continue;
      }
      let cell = cells.get(key);
      if (cell === undefined) {
        cell = {
          key,
          latitude: centre[0],
          longitude: centre[1],
          weight: 0,
          decodes: 0,
          callsigns: 0,
          bestFreqHz: 0,
          bestSnrDb: Number.NEGATIVE_INFINITY,
          measuredMuf3000Mhz: null,
          medianDistanceKm: 0,
          lastSeen: 0,
          calls: new Set<string>(),
          distances: [],
        };
        cells.set(key, cell);
      }
      cell.weight += weight;
      cell.decodes += 1;
      cell.calls.add(observation.callsign);
      cell.distances.push(observation.distanceKm);
      cell.bestFreqHz = Math.max(cell.bestFreqHz, observation.freqHz);
      cell.bestSnrDb = Math.max(cell.bestSnrDb, observation.snrDb);
      cell.lastSeen = Math.max(cell.lastSeen, observation.at);
      if (observation.muf3000Mhz !== null) {
        cell.measuredMuf3000Mhz = Math.max(
          cell.measuredMuf3000Mhz ?? Number.NEGATIVE_INFINITY,
          observation.muf3000Mhz,
        );
      }
    }
  }
  return [...cells.values()]
    .map(({ calls, distances, ...cell }) => ({
      ...cell,
      callsigns: calls.size,
      bestSnrDb: cell.bestSnrDb === Number.NEGATIVE_INFINITY ? 0 : cell.bestSnrDb,
      medianDistanceKm: median(distances),
    }))
    .toSorted((a, b) => b.weight - a.weight);
}

export function propagationPaths(
  observations: readonly PathObservation[],
  receiver: readonly [number, number],
  options: PropagationOptions,
  cap = 400,
): PropagationPath[] {
  const newest = new Map<string, PathObservation>();
  for (const observation of observations) {
    const key = `${observation.grid}|${Math.round(observation.freqHz / 1e5)}`;
    const held = newest.get(key);
    if (held === undefined || held.at < observation.at) {
      newest.set(key, observation);
    }
  }
  return [...newest.values()]
    .toSorted((a, b) => b.at - a.at)
    .slice(0, cap)
    .map((observation) => ({
      key: observation.key,
      from: [receiver[0], receiver[1]] as const,
      to: [observation.latitude, observation.longitude] as const,
      weight: decayWeight(options.nowMs - observation.at, options.halfLifeMinutes),
      freqHz: observation.freqHz,
    }));
}

export interface PropagationSummary {
  decodes: number;
  grids: number;
  callsigns: number;
  bands: number;
  bestFreqHz: number;
  bestMuf3000Mhz: number | null;
  farthestKm: number;
  oldest: number | null;
  newest: number | null;
}

export function propagationSummary(observations: readonly PathObservation[]): PropagationSummary {
  const grids = new Set<string>();
  const callsigns = new Set<string>();
  const bands = new Set<number>();
  let bestFreqHz = 0;
  let bestMuf: number | null = null;
  let farthestKm = 0;
  let oldest: number | null = null;
  let newest: number | null = null;
  for (const observation of observations) {
    grids.add(observation.grid);
    callsigns.add(observation.callsign);
    bands.add(Math.round(observation.freqHz / 1e5));
    bestFreqHz = Math.max(bestFreqHz, observation.freqHz);
    farthestKm = Math.max(farthestKm, observation.distanceKm);
    if (observation.muf3000Mhz !== null) {
      bestMuf = Math.max(bestMuf ?? Number.NEGATIVE_INFINITY, observation.muf3000Mhz);
    }
    oldest = oldest === null ? observation.at : Math.min(oldest, observation.at);
    newest = newest === null ? observation.at : Math.max(newest, observation.at);
  }
  return {
    decodes: observations.length,
    grids: grids.size,
    callsigns: callsigns.size,
    bands: bands.size,
    bestFreqHz,
    bestMuf3000Mhz: bestMuf,
    farthestKm,
    oldest,
    newest,
  };
}

export function receiverOf(fix: PositionFix | undefined): [number, number] | null {
  if (fix === undefined) {
    return null;
  }
  if (!Number.isFinite(fix.latitude) || !Number.isFinite(fix.longitude)) {
    return null;
  }
  return [fix.latitude, fix.longitude];
}

function median(values: readonly number[]): number {
  if (values.length === 0) {
    return 0;
  }
  const sorted = values.toSorted((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 1) {
    return sorted[middle] ?? 0;
  }
  return ((sorted[middle - 1] ?? 0) + (sorted[middle] ?? 0)) / 2;
}

export interface PropagationSession {
  observations: readonly PathObservation[];
  clearedAt: number;
}

interface PropagationStore {
  sessions: Record<string, PropagationSession>;
  observe: (node: string, observations: readonly PathObservation[]) => void;
  clear: (node: string, atMs?: number) => void;
}

export const EMPTY_SESSION: PropagationSession = { observations: [], clearedAt: 0 };

export const usePropagationStore = create<PropagationStore>((set) => ({
  sessions: {},
  observe: (node, observations) =>
    set((state) => {
      if (observations.length === 0) {
        return state;
      }
      const session = state.sessions[node] ?? EMPTY_SESSION;
      const merged = mergeObservations(session.observations, observations);
      if (merged === session.observations) {
        return state;
      }
      return {
        sessions: { ...state.sessions, [node]: { ...session, observations: merged } },
      };
    }),
  clear: (node, atMs = Date.now()) =>
    set((state) => ({
      sessions: { ...state.sessions, [node]: { observations: [], clearedAt: atMs } },
    })),
}));

export function mergeObservations(
  held: readonly PathObservation[],
  added: readonly PathObservation[],
  capacity = OBSERVATION_CAPACITY,
): readonly PathObservation[] {
  const seen = new Set(held.map((observation) => observation.key));
  const fresh = added.filter((observation) => !seen.has(observation.key));
  if (fresh.length === 0) {
    return held;
  }
  const merged = [...held, ...fresh].toSorted((a, b) => a.at - b.at);
  return merged.length > capacity ? merged.slice(merged.length - capacity) : merged;
}

export function liveObservations(
  observations: readonly PathObservation[],
  options: PropagationOptions,
  clearedAt = 0,
): PathObservation[] {
  const decayed = options.nowMs - options.halfLifeMinutes * 60_000 * DECAYS_KEPT;
  const cutoff = Math.max(clearedAt, decayed);
  return observations.filter((observation) => observation.at >= cutoff);
}
