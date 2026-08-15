import { create } from "zustand";
import type { SpectrumFrame } from "./frame";

export const SIGNAL_MIN_DBFS = -120;
export const SIGNAL_MAX_DBFS = -20;
const CELL_SIZE_M = 10;
const MAX_CELLS = 5_000;

export interface SignalSurveySample {
  latitude: number;
  longitude: number;
  levelDbfs: number;
  measuredAt: number;
  observations: number;
  accuracyM?: number;
}

export interface SignalSurveySession {
  recording: boolean;
  samples: readonly SignalSurveySample[];
}

interface SignalSurveyStore {
  sessions: Record<string, SignalSurveySession>;
  setRecording: (node: string, recording: boolean) => void;
  observe: (node: string, sample: Omit<SignalSurveySample, "observations">) => void;
  clear: (node: string) => void;
}

const EMPTY_SESSION: SignalSurveySession = { recording: false, samples: [] };

export const useSignalSurveyStore = create<SignalSurveyStore>((set) => ({
  sessions: {},
  setRecording: (node, recording) =>
    set((state) => ({
      sessions: {
        ...state.sessions,
        [node]: { ...(state.sessions[node] ?? EMPTY_SESSION), recording },
      },
    })),
  observe: (node, sample) =>
    set((state) => {
      const session = state.sessions[node] ?? EMPTY_SESSION;
      return {
        sessions: {
          ...state.sessions,
          [node]: { ...session, samples: mergeSurveySample(session.samples, sample) },
        },
      };
    }),
  clear: (node) =>
    set((state) => ({
      sessions: {
        ...state.sessions,
        [node]: { ...(state.sessions[node] ?? EMPTY_SESSION), samples: [] },
      },
    })),
}));

/** Peak spectrum level inside the requested RF slice. The value is relative to full scale;
 * converting it to dBm requires a receiver-specific calibration the spectrum stream does not
 * claim to have. */
export function measureSignalDbfs(
  frame: SpectrumFrame,
  frequencyHz: number,
  bandwidthHz: number,
): number | null {
  const count = frame.bins.length;
  if (
    count === 0 ||
    !(frame.spanHz > 0) ||
    !(frame.dbMax > frame.dbMin) ||
    !Number.isFinite(frequencyHz) ||
    !Number.isFinite(bandwidthHz) ||
    !(bandwidthHz > 0)
  ) {
    return null;
  }
  const frameLow = frame.centerHz - frame.spanHz / 2;
  const frameHigh = frame.centerHz + frame.spanHz / 2;
  if (frequencyHz < frameLow || frequencyHz > frameHigh) {
    return null;
  }

  const binHz = frame.spanHz / count;
  const sliceLow = Math.max(frameLow, frequencyHz - bandwidthHz / 2);
  const sliceHigh = Math.min(frameHigh, frequencyHz + bandwidthHz / 2);
  const first = Math.max(0, Math.min(count - 1, Math.floor((sliceLow - frameLow) / binHz)));
  const last = Math.max(first, Math.min(count - 1, Math.ceil((sliceHigh - frameLow) / binHz) - 1));
  let peak = 0;
  for (let index = first; index <= last; index += 1) {
    peak = Math.max(peak, frame.bins[index] ?? 0);
  }
  return frame.dbMin + (peak / 255) * (frame.dbMax - frame.dbMin);
}

/** One point per roughly ten-metre cell: stopping at a traffic light must not make that location
 * look stronger merely because it produced more fixes than the road around it. Repeated readings
 * are averaged in linear power, then converted back to dB. */
export function mergeSurveySample(
  samples: readonly SignalSurveySample[],
  incoming: Omit<SignalSurveySample, "observations">,
): readonly SignalSurveySample[] {
  const key = cellKey(incoming.latitude, incoming.longitude);
  const index = samples.findIndex((sample) => cellKey(sample.latitude, sample.longitude) === key);
  if (index < 0) {
    const next = [...samples, { ...incoming, observations: 1 }];
    return next.length > MAX_CELLS ? next.slice(next.length - MAX_CELLS) : next;
  }

  const previous = samples[index];
  if (previous === undefined) {
    return samples;
  }
  const observations = previous.observations + 1;
  const meanPower =
    (dbToPower(previous.levelDbfs) * previous.observations + dbToPower(incoming.levelDbfs)) /
    observations;
  const merged: SignalSurveySample = {
    latitude: (previous.latitude * previous.observations + incoming.latitude) / observations,
    longitude: (previous.longitude * previous.observations + incoming.longitude) / observations,
    levelDbfs: 10 * Math.log10(meanPower),
    measuredAt: incoming.measuredAt,
    observations,
    ...(incoming.accuracyM === undefined ? {} : { accuracyM: incoming.accuracyM }),
  };
  return samples.with(index, merged);
}

export function signalSurveyCsv(
  samples: readonly SignalSurveySample[],
  frequencyHz: number,
  bandwidthHz: number,
): string {
  const rows = samples.map((sample) =>
    [
      new Date(sample.measuredAt).toISOString(),
      frequencyHz,
      bandwidthHz,
      sample.latitude.toFixed(7),
      sample.longitude.toFixed(7),
      sample.accuracyM?.toFixed(1) ?? "",
      sample.levelDbfs.toFixed(2),
      sample.observations,
    ].join(","),
  );
  return [
    "time,frequency_hz,bandwidth_hz,latitude,longitude,accuracy_m,level_dbfs,observations",
    ...rows,
  ].join("\n");
}

function cellKey(latitude: number, longitude: number): string {
  const latitudeRad = (latitude * Math.PI) / 180;
  const x = longitude * 111_320 * Math.max(0.01, Math.cos(latitudeRad));
  const y = latitude * 110_540;
  return `${Math.round(x / CELL_SIZE_M)}:${Math.round(y / CELL_SIZE_M)}`;
}

function dbToPower(db: number): number {
  return 10 ** (db / 10);
}
