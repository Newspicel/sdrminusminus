// The playback AudioWorklet, loaded from a Blob URL so it works identically under Vite dev
// and build (no separate worklet asset to ship). The processor is a thin shell: all
// scheduling lives in JitterBuffer, whose transpiled source is injected via toString() —
// which is why that class must stay self-contained.
import { JitterBuffer } from "./jitter";

export const PROCESSOR_NAME = "sdr-audio-playback";

export const SAMPLE_RATE = 48_000;
/**
 * The graph is always two-channel: a channel can switch between mono and stereo mid-stream
 * (WFM's stereo toggle), and rebuilding the node — and its buffered audio — for that would
 * cost a gap. Mono streams are duplicated into both channels on the way in instead.
 */
export const CHANNELS = 2;
/** ~100 ms pre-buffer (PLAN §9: 60–100 ms jitter buffer), in sample frames. */
export const TARGET_FRAMES = 4_800;
/** ~400 ms cap; a burst past it (tab sleep) sheds back to `TARGET_FRAMES`, not to the cap. */
export const MAX_FRAMES = 19_200;

/**
 * Port protocol: Float32Array = interleaved PCM at `CHANNELS`, "reset" = clear buffer,
 * "close" = end the processor.
 */
export type WorkletMessage = Float32Array | "reset" | "close";

const processorSource = `
"use strict";
const JitterBuffer = ${JitterBuffer.toString()};
class PlaybackProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { targetFrames, maxFrames, channels } = options.processorOptions;
    this.jitter = new JitterBuffer(targetFrames, maxFrames, channels);
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
    const channels = outputs[0];
    if (channels && channels.length > 0) {
      this.jitter.read(channels);
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
