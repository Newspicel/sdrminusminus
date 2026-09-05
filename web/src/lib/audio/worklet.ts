import { JitterBuffer } from "./jitter";

export const PROCESSOR_NAME = "sdr-audio-playback";

export const SAMPLE_RATE = 48_000;
export const CHANNELS = 2;
export const TARGET_FRAMES = 4_800;
export const MAX_FRAMES = 19_200;

export function targetFramesForHost(hostname: string): number {
  return ["localhost", "127.0.0.1", "[::1]", "::1", "tauri.localhost"].includes(hostname)
    ? 2_880
    : TARGET_FRAMES;
}
export const MAX_GAP_FRAMES = 19_200;

export type WorkletMessage = Float32Array | "reset" | "close";

export interface WorkletReport {
  underruns: number;
  bufferedFrames?: number;
  targetFrames?: number;
  trimmedFrames?: number;
  decoderDroppedFrames?: number;
}

const processorSource = `
"use strict";
const JitterBuffer = ${JitterBuffer.toString()};
class PlaybackProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { targetFrames, maxFrames, channels } = options.processorOptions;
    this.jitter = new JitterBuffer(targetFrames, maxFrames, channels);
    this.ended = false;
    this.reported = 0;
    this.reportAfter = 0;
    this.port.onmessage = (event) => {
      const data = event.data;
      if (data === "close") {
        this.ended = true;
      } else if (data === "reset") {
        this.jitter.clear();
      } else {
        this.jitter.push(data);
      }
    };
  }
  process(inputs, outputs) {
    const channels = outputs[0];
    if (channels && channels.length > 0) {
      this.jitter.read(channels);
      this.reportAfter += channels[0].length;
      if (this.jitter.underruns !== this.reported || this.reportAfter >= ${SAMPLE_RATE / 2}) {
        this.reportAfter = 0;
        this.reported = this.jitter.underruns;
        this.port.postMessage({ underruns: this.reported, bufferedFrames: this.jitter.buffered, targetFrames: this.jitter.targetDepth, trimmedFrames: this.jitter.trimmed });
      }
    }
    return !this.ended;
  }
}
registerProcessor(${JSON.stringify(PROCESSOR_NAME)}, PlaybackProcessor);
`;

let url: string | null = null;

export function processorUrl(): string {
  url ??= URL.createObjectURL(new Blob([processorSource], { type: "text/javascript" }));
  return url;
}
