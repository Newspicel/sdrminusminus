// Opus packet decoding (PLAN §9): WebCodecs `AudioDecoder` when the platform supports the
// exact stream config, else the "opus-decoder" WASM build (the likely WKWebView/Tauri path).
// Both paths emit interleaved Float32 PCM at 48 kHz, in the channel count they were built for.
import { SAMPLE_RATE } from "./worklet";

export interface OpusPacketDecoder {
  /** Channel count this decoder was configured for; interleave of what it emits. */
  readonly channels: number;
  decode(packet: Uint8Array, timestampUs: number): void;
  close(): void;
}

// Opus decode is far faster than realtime, so a growing WebCodecs queue means the pipeline
// is stuck; drop instead of queueing unboundedly (PLAN §5: UI streams are drop-oldest).
const MAX_DECODE_QUEUE = 16;

function config(channels: number): AudioDecoderConfig {
  return { codec: "opus", sampleRate: SAMPLE_RATE, numberOfChannels: channels };
}

// Support is per stream config, so the probe is keyed by channel count: a platform may do
// mono and not stereo, and answering from a mono probe would configure a decoder that throws.
const webCodecsProbes = new Map<number, Promise<boolean>>();

function webCodecsSupported(channels: number): Promise<boolean> {
  let probe = webCodecsProbes.get(channels);
  if (!probe) {
    probe = (async () => {
      if (typeof AudioDecoder === "undefined") {
        return false;
      }
      try {
        const support = await AudioDecoder.isConfigSupported(config(channels));
        return support.supported === true;
      } catch {
        return false;
      }
    })();
    webCodecsProbes.set(channels, probe);
  }
  return probe;
}

export async function createOpusPacketDecoder(
  channels: number,
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
): Promise<OpusPacketDecoder> {
  if (await webCodecsSupported(channels)) {
    return createWebCodecsDecoder(channels, onPcm, onError);
  }
  return createWasmDecoder(channels, onPcm, onError);
}

function createWebCodecsDecoder(
  channels: number,
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
): OpusPacketDecoder {
  const decoder = new AudioDecoder({
    output: (data) => {
      try {
        // Planar copy per plane, then interleave here: `copyTo`'s conversion to interleaved
        // f32 is not implemented consistently across engines, planar always is.
        const pcm = new Float32Array(data.numberOfFrames * channels);
        const plane = new Float32Array(data.numberOfFrames);
        for (let c = 0; c < channels; c++) {
          data.copyTo(plane, { planeIndex: c, format: "f32-planar" });
          for (let f = 0; f < plane.length; f++) {
            pcm[f * channels + c] = plane[f] ?? 0;
          }
        }
        onPcm(pcm);
      } finally {
        data.close();
      }
    },
    error: onError,
  });
  decoder.configure(config(channels));
  return {
    channels,
    decode(packet, timestampUs) {
      if (decoder.state !== "configured" || decoder.decodeQueueSize > MAX_DECODE_QUEUE) {
        return;
      }
      // Every Opus packet is independently submittable, so "key" is always correct.
      decoder.decode(new EncodedAudioChunk({ type: "key", timestamp: timestampUs, data: packet }));
    },
    close() {
      if (decoder.state !== "closed") {
        decoder.close();
      }
    },
  };
}

async function createWasmDecoder(
  channels: number,
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
): Promise<OpusPacketDecoder> {
  // Dynamic import so the WASM payload is only fetched when WebCodecs can't do Opus.
  const { OpusDecoder } = await import("opus-decoder");
  // RFC 7845 §5.1.1 channel mapping family 0: one stream, coupled for stereo.
  const decoder = new OpusDecoder({
    channels,
    streamCount: 1,
    coupledStreamCount: channels === 2 ? 1 : 0,
    channelMappingTable: channels === 2 ? [0, 1] : [0],
  });
  await decoder.ready;
  let closed = false;
  return {
    channels,
    decode(packet) {
      if (closed) {
        return;
      }
      try {
        const { channelData, samplesDecoded, errors } = decoder.decodeFrame(packet);
        if (errors.length > 0) {
          onError(new Error(errors.map((e) => e.message).join("; ")));
          return;
        }
        if (samplesDecoded <= 0) {
          return;
        }
        // The decoder owns its planar buffers, so the interleave doubles as the copy the
        // worklet transfer needs.
        const pcm = new Float32Array(samplesDecoded * channels);
        for (let c = 0; c < channels; c++) {
          const plane = channelData[c];
          if (!plane) {
            return;
          }
          for (let f = 0; f < samplesDecoded; f++) {
            pcm[f * channels + c] = plane[f] ?? 0;
          }
        }
        onPcm(pcm);
      } catch (err) {
        onError(err);
      }
    },
    close() {
      closed = true;
      decoder.free();
    },
  };
}
