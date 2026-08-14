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
/**
 * ~100 ms pre-buffer (PLAN §9: 60–100 ms jitter buffer), in sample frames. It is the floor
 * the buffer adapts up from when a path underruns, not a fixed depth.
 */
export const TARGET_FRAMES = 4_800;
/**
 * ~1 s ring: room for the adaptive target's ceiling (3× the floor) plus jitter on top of it.
 * A burst past the cap (tab sleep) sheds back to the target, not to the cap.
 */
export const MAX_FRAMES = 48_000;
/**
 * Largest hole in the packet clock that is concealed with silence rather than restarted from
 * (~400 ms). Concealing keeps timing honest; past this the stream is better rebuffered than
 * padded, and the pad itself would be latency the buffer then has to walk back off.
 */
export const MAX_GAP_FRAMES = 19_200;

/**
 * Port protocol, main → worklet: Float32Array = interleaved PCM at `CHANNELS`, "reset" = clear
 * buffer, "close" = end the processor.
 */
export type WorkletMessage = Float32Array | "reset" | "close";

/**
 * Port protocol, worklet → main: the running underrun count, posted only when it changes.
 * Playback running dry is the one thing the audio thread knows and nothing upstream can infer —
 * it is what separates "the audio never arrived" from "it arrived and we could not play it".
 */
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
