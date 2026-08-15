import { JitterBuffer } from "./jitter";

export const PROCESSOR_NAME = "sdr-audio-playback";

export const SAMPLE_RATE = 48_000;
export const CHANNELS = 2;
export const TARGET_FRAMES = 4_800;
export const MAX_FRAMES = 48_000;
export const MAX_GAP_FRAMES = 19_200;

export type WorkletMessage = Float32Array | "reset" | "close";

export interface WorkletReport {
  underruns: number;
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
      // Only on change, so the steady state posts nothing at all from the audio thread.
      if (this.jitter.underruns !== this.reported) {
        this.reported = this.jitter.underruns;
        this.port.postMessage({ underruns: this.reported });
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
