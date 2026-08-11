// The WebAudio half of a channel: Opus decoder → worklet jitter buffer → per-channel gain.
// One shared AudioContext for all channels; mixing is just parallel graphs (PLAN §9).
import type { OpusPacketDecoder } from "./decoder";
import { createOpusPacketDecoder } from "./decoder";
import type { AudioSink, SinkFactory } from "./engine";
import type { WorkletMessage } from "./worklet";
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

/** Whether the shared context is actually producing sound (autoplay/interruption aware). */
export function isOutputRunning(): boolean {
  return ctx === null || ctx.state === "running";
}

export function onOutputStateChange(listener: (running: boolean) => void): () => void {
  outputListeners.add(listener);
  return () => outputListeners.delete(listener);
}

/** Must be called from a gesture handler: iOS only allows resume() inside a user gesture. */
export function resumeAudioOutput(): void {
  attemptResume();
}

function attemptResume(): void {
  if (ctx === null || ctx.state === "running" || ctx.state === "closed") {
    return;
  }
  // A rejected resume is not silent: the state stays non-running, so the armed retries and
  // the engine's "suspended" UI state remain in effect until a later attempt succeeds.
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

// While suspended/interrupted (phone call, Siri, autoplay veto — WebKit reports states
// beyond "suspended"), retry on the signals that can legally un-suspend the context: a
// fresh user gesture or the tab becoming visible again.
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

/**
 * Spread `pcm` (interleaved at `channels`) across the graph's fixed `CHANNELS` output: a mono
 * stream plays on both sides rather than only on the left.
 */
function toOutputLayout(pcm: Float32Array, channels: number): Float32Array {
  if (channels === CHANNELS) {
    return pcm;
  }
  const frames = Math.floor(pcm.length / channels);
  const out = new Float32Array(frames * CHANNELS);
  for (let f = 0; f < frames; f++) {
    for (let c = 0; c < CHANNELS; c++) {
      // Fewer source channels than outputs: the last one feeds the rest.
      out[f * CHANNELS + c] = pcm[f * channels + Math.min(c, channels - 1)] ?? 0;
    }
  }
  return out;
}

export const createWebAudioSink: SinkFactory = async (volume, onError) => {
  // Runs synchronously inside the user's start() gesture — the standard autoplay unlock:
  // both context creation and resume() must happen before the first await.
  if (ctx === null) {
    ctx = new AudioContext({ sampleRate: SAMPLE_RATE });
    ctx.addEventListener("statechange", handleStateChange);
  }
  const context = ctx;
  if (context.state !== "running") {
    attemptResume();
    // Publish immediately: if the resume is vetoed, "suspended" must show without waiting
    // for a statechange that may never fire.
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

  let closed = false;
  // Bound into each decoder's callback, never read from here: a late frame from the decoder
  // being replaced must be spread with the layout it was decoded in, not the incoming one.
  const emit = (channels: number) => (pcm: Float32Array) => {
    if (!closed) {
      post(node, toOutputLayout(pcm, channels));
    }
  };

  let decoder: OpusPacketDecoder;
  try {
    // Mono up front so a decoder that cannot be built at all still fails the sink (and the
    // channel) here, rather than silently on the first packet. A stereo stream swaps below.
    decoder = await createOpusPacketDecoder(1, emit(1), onError);
  } catch (err) {
    // The worklet processor only ends on "close"; without this teardown the node keeps
    // running on the audio thread forever while the engine holds no sink handle to close.
    post(node, "close");
    node.disconnect();
    gain.disconnect();
    throw err;
  }

  // A channel may change layout mid-stream (WFM stereo toggled), which no Opus decoder can
  // be reconfigured for: build the replacement, then swap. Packets that arrive meanwhile are
  // dropped rather than decoded with the wrong channel count — the client hears the same
  // few-frame gap the encoder's own layout switch already leaves.
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
        return;
      }
      if (channels !== decoder.channels) {
        useLayout(channels);
        return;
      }
      decoder.decode(opus, timestampUs);
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
      node.disconnect();
      gain.disconnect();
    },
  };
  return sink;
};
