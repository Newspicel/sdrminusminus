// The playback AudioWorklet, loaded from a Blob URL so it works identically under Vite dev
// and build (no separate worklet asset to ship). The processor is a thin shell: all
// scheduling lives in JitterBuffer, whose transpiled source is injected via toString() —
// which is why that class must stay self-contained.
import { JitterBuffer } from "./jitter";

export const PROCESSOR_NAME = "sdr-audio-playback";

export const SAMPLE_RATE = 48_000;
/** ~100 ms pre-buffer (PLAN §9: 60–100 ms jitter buffer). */
export const TARGET_SAMPLES = 4_800;
/** ~400 ms cap; a burst past it (tab sleep) sheds back to `TARGET_SAMPLES`, not to the cap. */
export const MAX_SAMPLES = 19_200;

/** Port protocol: Float32Array = PCM, "reset" = clear buffer, "close" = end the processor. */
export type WorkletMessage = Float32Array | "reset" | "close";

const processorSource = `
"use strict";
const JitterBuffer = ${JitterBuffer.toString()};
class PlaybackProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { targetSamples, maxSamples } = options.processorOptions;
    this.jitter = new JitterBuffer(targetSamples, maxSamples);
    this.ended = false;
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
    const channel = outputs[0][0];
    if (channel) {
      this.jitter.read(channel);
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
