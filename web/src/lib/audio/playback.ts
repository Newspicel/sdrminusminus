import { JitterBuffer } from "./jitter";
import type { WorkletMessage, WorkletReport } from "./worklet";
import {
  CHANNELS,
  MAX_FRAMES,
  PROCESSOR_NAME,
  processorUrl,
  SAMPLE_RATE,
  TARGET_FRAMES,
} from "./worklet";

const FALLBACK_BLOCK_FRAMES = 2048;

class OutputReadiness {
  readonly ready: Promise<void>;
  private begin: (() => void) | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private closed = false;

  constructor(context: AudioContext) {
    const initial = context.currentTime;
    const outputTime = (): number =>
      typeof context.getOutputTimestamp === "function"
        ? (context.getOutputTimestamp().contextTime ?? 0)
        : context.currentTime;
    const initialOutput = outputTime();
    const minimumProgress = TARGET_FRAMES / SAMPLE_RATE;
    this.ready = new Promise<void>((resolve) => {
      const check = (): void => {
        if (this.closed) return;
        if (
          context.currentTime - initial >= minimumProgress &&
          outputTime() - initialOutput >= minimumProgress
        ) {
          resolve();
        } else {
          this.timer = setTimeout(check, 10);
        }
      };
      this.begin = check;
    });
  }

  render(): void {
    const begin = this.begin;
    this.begin = undefined;
    begin?.();
  }

  close(): void {
    this.closed = true;
    clearTimeout(this.timer);
  }
}

export interface Playback {
  readonly node: AudioNode;
  readonly ready: Promise<void>;
  send(message: WorkletMessage): void;
  release(): void;
}

let workletModule: Promise<void> | null = null;

export function supportsAudioWorklet(context: BaseAudioContext): boolean {
  return (
    typeof AudioWorkletNode === "function" && typeof context.audioWorklet?.addModule === "function"
  );
}

export async function createPlayback(
  context: AudioContext,
  onReport: (report: WorkletReport) => void,
  onError: (error: Error) => void,
): Promise<Playback> {
  if (!supportsAudioWorklet(context)) {
    return createScriptProcessorPlayback(context, onReport);
  }
  workletModule ??= context.audioWorklet.addModule(processorUrl()).catch((err: unknown) => {
    workletModule = null;
    throw err;
  });
  await workletModule;
  return createWorkletPlayback(context, onReport, onError);
}

function createWorkletPlayback(
  context: AudioContext,
  onReport: (report: WorkletReport) => void,
  onError: (error: Error) => void,
): Playback {
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
  const readiness = new OutputReadiness(context);
  node.port.onmessage = (event: MessageEvent<WorkletReport>) => {
    readiness.render();
    onReport(event.data);
  };
  node.onprocessorerror = () => onError(new Error("Audio playback processor failed"));
  return {
    node,
    ready: readiness.ready,
    send(message) {
      if (message instanceof Float32Array) {
        node.port.postMessage(message, [message.buffer]);
      } else {
        // oxlint-disable-next-line unicorn/require-post-message-target-origin -- MessagePort.postMessage takes no targetOrigin (that's window.postMessage).
        node.port.postMessage(message);
      }
    },
    release() {
      readiness.close();
      node.onprocessorerror = null;
      node.port.onmessage = null;
      node.disconnect();
    },
  };
}

function createScriptProcessorPlayback(
  context: AudioContext,
  onReport: (report: WorkletReport) => void,
): Playback {
  const jitter = new JitterBuffer(TARGET_FRAMES, MAX_FRAMES, CHANNELS);
  // oxlint-disable-next-line no-deprecated -- AudioWorklet is secure-context only, so a plain-HTTP origin has nothing else to pull samples with.
  const node = context.createScriptProcessor(FALLBACK_BLOCK_FRAMES, 0, CHANNELS);
  const lanes: Float32Array[] = [];
  let ended = false;
  let reported = 0;
  let sinceReport = 0;
  const readiness = new OutputReadiness(context);
  node.onaudioprocess = (event: AudioProcessingEvent) => {
    readiness.render();
    const output = event.outputBuffer;
    lanes.length = 0;
    for (let channel = 0; channel < output.numberOfChannels; channel++) {
      lanes.push(output.getChannelData(channel));
    }
    if (ended) {
      for (const lane of lanes) {
        lane.fill(0);
      }
      return;
    }
    jitter.read(lanes);
    sinceReport += output.length;
    if (jitter.underruns !== reported || sinceReport >= SAMPLE_RATE / 2) {
      sinceReport = 0;
      reported = jitter.underruns;
      onReport({
        underruns: reported,
        bufferedFrames: jitter.buffered,
        targetFrames: jitter.targetDepth,
        trimmedFrames: jitter.trimmed,
      });
    }
  };
  return {
    node,
    ready: readiness.ready,
    send(message) {
      if (message === "close") {
        ended = true;
      } else if (message === "reset") {
        jitter.clear();
      } else {
        jitter.push(message);
      }
    },
    release() {
      readiness.close();
      node.onaudioprocess = null;
      node.disconnect();
    },
  };
}
