import { useEffect, useState, useSyncExternalStore } from "react";
import { watchAudio } from "../../lib/audio/monitor";
import { audioEngine } from "../../lib/audio/useChannelAudio";
import type { Input } from "../binding";

const LEVEL_TICK_MS = 100;
const FRAMES_PER_MS = 48;

export function pcmPeakDb(pcm: Float32Array): number {
  let peak = 0;
  for (const sample of pcm) {
    peak = Math.max(peak, Math.abs(sample));
  }
  return peak > 0 ? 20 * Math.log10(peak) : Number.NEGATIVE_INFINITY;
}

export function useAudioPeakDb(source: string, playing: boolean): number | undefined {
  const [peakDb, setPeakDb] = useState<number | undefined>(undefined);
  useEffect(() => {
    if (!playing) {
      return;
    }
    let held = Number.NEGATIVE_INFINITY;
    const stop = watchAudio(source, (pcm) => {
      held = Math.max(held, pcmPeakDb(pcm));
    });
    const timer = setInterval(() => {
      setPeakDb(held);
      held = Number.NEGATIVE_INFINITY;
    }, LEVEL_TICK_MS);
    return () => {
      stop();
      clearInterval(timer);
    };
  }, [source, playing]);
  return playing ? peakDb : undefined;
}

export interface SpeakerHealth {
  bufferedMs: number;
  trimmedMs: number;
  droppedMs: number;
  underruns: number;
  suspended: boolean;
}

type Reading = (deviceSet: number, channel: number, fx: readonly string[]) => number;

function total(inputs: readonly Input[], read: Reading): () => number {
  return () =>
    inputs.reduce((sum, input) => sum + read(input.deviceSet, input.channel.id, input.fx), 0);
}

export function useSpeakerHealth(inputs: readonly Input[], dev: boolean): SpeakerHealth {
  const subscribe = audioEngine.subscribe;
  const lostFrames = useSyncExternalStore(
    subscribe,
    total(inputs, (d, c, fx) => audioEngine.getLostFrames(d, c, fx)),
  );
  const underruns = useSyncExternalStore(
    subscribe,
    total(inputs, (d, c, fx) => audioEngine.getUnderruns(d, c, fx)),
  );
  const bufferedMs = useSyncExternalStore(
    subscribe,
    total(inputs, (d, c, fx) => audioEngine.getBufferedMs(d, c, fx)),
  );
  const trimmedMs = useSyncExternalStore(
    subscribe,
    total(inputs, (d, c, fx) => audioEngine.getTrimmedMs(d, c, fx)),
  );
  const suspended = useSyncExternalStore(subscribe, () =>
    inputs.some((input) => audioEngine.isSuspended(input.deviceSet, input.channel.id, input.fx)),
  );
  return {
    bufferedMs: dev ? bufferedMs : 0,
    trimmedMs: dev ? trimmedMs : 0,
    droppedMs: lostFrames / FRAMES_PER_MS,
    underruns,
    suspended,
  };
}

export function speakerHealthShown(health: SpeakerHealth): boolean {
  return (
    health.suspended ||
    health.bufferedMs > 0 ||
    health.trimmedMs > 0 ||
    health.droppedMs > 0 ||
    health.underruns > 0
  );
}
