import { registerMediaLatency } from "../mediaLatency";
import type { OpusPacketDecoder } from "./decoder";
import { createOpusPacketDecoder } from "./decoder";
import type { AudioSink, SinkFactory } from "./engine";
import { isWatched, publishAudio } from "./monitor";
import { createPlayback } from "./playback";
import type { WorkletReport } from "./worklet";
import { CHANNELS, SAMPLE_RATE, targetFramesForHost } from "./worklet";

const VOLUME_RANGE_DB = 60;
const VOLUME_RAMP_SECONDS = 0.02;

let ctx: AudioContext | null = null;
const outputListeners = new Set<(running: boolean) => void>();
let recoveryArmed = false;

export function gainForVolume(volume: number): number {
  const position = Math.min(1, Math.max(0, volume));
  return position <= 0 ? 0 : 10 ** ((position - 1) * (VOLUME_RANGE_DB / 20));
}

export function isOutputRunning(): boolean {
  return ctx === null || ctx.state === "running";
}

export function onOutputStateChange(listener: (running: boolean) => void): () => void {
  outputListeners.add(listener);
  return () => outputListeners.delete(listener);
}

export function resumeAudioOutput(): void {
  attemptResume();
}

function attemptResume(): void {
  if (ctx === null || ctx.state === "running" || ctx.state === "closed") {
    return;
  }
  ctx.resume().catch(() => {});
}

function handleStateChange(): void {
  const running = isOutputRunning();
  if (running) {
    disarmRecovery();
  } else {
    armRecovery();
  }
  for (const listener of outputListeners) {
    listener(running);
  }
}

function armRecovery(): void {
  if (recoveryArmed) {
    return;
  }
  recoveryArmed = true;
  document.addEventListener("pointerdown", attemptResume, true);
  document.addEventListener("visibilitychange", handleVisibility);
}

function disarmRecovery(): void {
  if (!recoveryArmed) {
    return;
  }
  recoveryArmed = false;
  document.removeEventListener("pointerdown", attemptResume, true);
  document.removeEventListener("visibilitychange", handleVisibility);
}

function handleVisibility(): void {
  if (document.visibilityState === "visible") {
    attemptResume();
  }
}

export const createWebAudioSink: SinkFactory = async (key, volume, onError, onReport) => {
  if (ctx === null) {
    ctx = new AudioContext({ sampleRate: SAMPLE_RATE });
    ctx.addEventListener("statechange", handleStateChange);
  }
  const context = ctx;
  if (context.state !== "running") {
    attemptResume();
    handleStateChange();
  }
  let lastReport: WorkletReport = { underruns: 0 };
  let decoderDroppedFrames = 0;
  const onDropped = (frames: number): void => {
    decoderDroppedFrames += frames;
    onReport({ ...lastReport, decoderDroppedFrames });
  };
  const playback = await createPlayback(
    context,
    (report) => {
      lastReport = report;
      onReport({ ...lastReport, decoderDroppedFrames });
    },
    onError,
  );
  const gain = new GainNode(context, { gain: gainForVolume(volume) });
  playback.node.connect(gain).connect(context.destination);

  let closed = false;
  let received = false;
  const removeLatency = registerMediaLatency(key, () => {
    if (!received || closed || context.state !== "running") return 0;
    const frames =
      lastReport.bufferedFrames ??
      targetFramesForHost(typeof location === "undefined" ? "" : location.hostname);
    return (
      1000 * (frames / SAMPLE_RATE + (context.baseLatency ?? 0) + (context.outputLatency ?? 0))
    );
  });
  const emit = (pcm: Float32Array): void => {
    if (closed) {
      return;
    }
    if (isWatched(key)) {
      publishAudio(key, pcm, CHANNELS);
    }
    received = true;
    playback.send(pcm);
  };

  let decoder: OpusPacketDecoder;
  try {
    decoder = await createOpusPacketDecoder(CHANNELS, emit, onError, onDropped);
  } catch (err) {
    playback.send("close");
    playback.release();
    gain.disconnect();
    removeLatency();
    throw err;
  }

  const sink: AudioSink = {
    ready: playback.ready,
    push(opus, timestampUs, channels) {
      if (closed) {
        return false;
      }
      if (channels !== 1 && channels !== 2) {
        onError(new Error(`Unsupported audio channel count: ${channels}`));
        return false;
      }
      return decoder.decode(opus, timestampUs);
    },
    conceal(frames) {
      playback.send(new Float32Array(frames * CHANNELS));
    },
    setVolume(v) {
      gain.gain.setTargetAtTime(gainForVolume(v), context.currentTime, VOLUME_RAMP_SECONDS);
    },
    reset() {
      received = false;
      decoder.reset();
      playback.send("reset");
    },
    close() {
      closed = true;
      removeLatency();
      decoder.close();
      playback.send("close");
      playback.release();
      gain.disconnect();
    },
  };
  return sink;
};
