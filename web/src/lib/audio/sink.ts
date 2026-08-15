import type { OpusPacketDecoder } from "./decoder";
import { createOpusPacketDecoder } from "./decoder";
import type { AudioSink, SinkFactory } from "./engine";
import { isWatched, publishAudio } from "./monitor";
import type { WorkletMessage, WorkletReport } from "./worklet";
import {
  CHANNELS,
  MAX_FRAMES,
  PROCESSOR_NAME,
  processorUrl,
  SAMPLE_RATE,
  TARGET_FRAMES,
} from "./worklet";

let ctx: AudioContext | null = null;
let workletModule: Promise<void> | null = null;
const outputListeners = new Set<(running: boolean) => void>();
let recoveryArmed = false;

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

function post(node: AudioWorkletNode, message: WorkletMessage): void {
  if (message instanceof Float32Array) {
    node.port.postMessage(message, [message.buffer]);
  } else {
    // oxlint-disable-next-line unicorn/require-post-message-target-origin -- MessagePort.postMessage takes no targetOrigin (that's window.postMessage).
    node.port.postMessage(message);
  }
}

function toOutputLayout(pcm: Float32Array, channels: number): Float32Array {
  if (channels === CHANNELS) {
    return pcm;
  }
  const frames = Math.floor(pcm.length / channels);
  const out = new Float32Array(frames * CHANNELS);
  for (let f = 0; f < frames; f++) {
    for (let c = 0; c < CHANNELS; c++) {
      out[f * CHANNELS + c] = pcm[f * channels + Math.min(c, channels - 1)] ?? 0;
    }
  }
  return out;
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
  workletModule ??= context.audioWorklet.addModule(processorUrl()).catch((err: unknown) => {
    workletModule = null;
    throw err;
  });
  await workletModule;

  const node = new AudioWorkletNode(context, PROCESSOR_NAME, {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [CHANNELS],
    processorOptions: {
      targetFrames: TARGET_FRAMES,
      maxFrames: MAX_FRAMES,
      channels: CHANNELS,
    },
  });
  const gain = new GainNode(context, { gain: volume });
  node.connect(gain).connect(context.destination);
  node.port.onmessage = (event: MessageEvent<WorkletReport>) => {
    onReport(event.data);
  };

  let closed = false;
  const emit = (channels: number) => (pcm: Float32Array) => {
    if (closed) {
      return;
    }
    if (isWatched(key)) {
      publishAudio(key, pcm, channels);
    }
    post(node, toOutputLayout(pcm, channels));
  };

  let decoder: OpusPacketDecoder;
  try {
    decoder = await createOpusPacketDecoder(1, emit(1), onError);
  } catch (err) {
    post(node, "close");
    node.disconnect();
    gain.disconnect();
    throw err;
  }

  let swapping = false;
  const useLayout = (channels: number): void => {
    if (closed || swapping || channels === decoder.channels) {
      return;
    }
    swapping = true;
    createOpusPacketDecoder(channels, emit(channels), onError)
      .then((next) => {
        swapping = false;
        if (closed) {
          next.close();
          return;
        }
        decoder.close();
        decoder = next;
      })
      .catch((err: unknown) => {
        swapping = false;
        onError(err);
      });
  };

  const sink: AudioSink = {
    push(opus, timestampUs, channels) {
      if (closed) {
        return false;
      }
      if (channels !== decoder.channels) {
        useLayout(channels);
        return false;
      }
      return decoder.decode(opus, timestampUs);
    },
    conceal(frames) {
      post(node, new Float32Array(frames * CHANNELS));
    },
    setVolume(v) {
      gain.gain.value = v;
    },
    reset() {
      post(node, "reset");
    },
    close() {
      closed = true;
      decoder.close();
      post(node, "close");
      node.port.onmessage = null;
      node.disconnect();
      gain.disconnect();
    },
  };
  return sink;
};
