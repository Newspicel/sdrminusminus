// Opus packet decoding (PLAN §9): WebCodecs `AudioDecoder` when the platform supports the
// exact stream config, else the "opus-decoder" WASM build (the likely WKWebView/Tauri path).
// Both paths emit Float32 PCM at 48 kHz mono.
import { SAMPLE_RATE } from "./worklet";

export interface OpusPacketDecoder {
  decode(packet: Uint8Array, timestampUs: number): void;
  close(): void;
}

const OPUS_CONFIG: AudioDecoderConfig = {
  codec: "opus",
  sampleRate: SAMPLE_RATE,
  numberOfChannels: 1,
};

// Opus decode is far faster than realtime, so a growing WebCodecs queue means the pipeline
// is stuck; drop instead of queueing unboundedly (PLAN §5: UI streams are drop-oldest).
const MAX_DECODE_QUEUE = 16;

let webCodecsProbe: Promise<boolean> | null = null;

function webCodecsSupported(): Promise<boolean> {
  webCodecsProbe ??= (async () => {
    if (typeof AudioDecoder === "undefined") {
      return false;
    }
    try {
      const support = await AudioDecoder.isConfigSupported(OPUS_CONFIG);
      return support.supported === true;
    } catch {
      return false;
    }
  })();
  return webCodecsProbe;
}

export async function createOpusPacketDecoder(
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
): Promise<OpusPacketDecoder> {
  if (await webCodecsSupported()) {
    return createWebCodecsDecoder(onPcm, onError);
  }
  return createWasmDecoder(onPcm, onError);
}

function createWebCodecsDecoder(
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
): OpusPacketDecoder {
  const decoder = new AudioDecoder({
    output: (data) => {
      try {
        const pcm = new Float32Array(data.numberOfFrames);
        data.copyTo(pcm, { planeIndex: 0, format: "f32-planar" });
        onPcm(pcm);
      } finally {
        data.close();
      }
    },
    error: onError,
  });
  decoder.configure(OPUS_CONFIG);
  return {
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
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
): Promise<OpusPacketDecoder> {
  // Dynamic import so the WASM payload is only fetched when WebCodecs can't do Opus.
  const { OpusDecoder } = await import("opus-decoder");
  // RFC 7845 §5.1.1 mono mapping: one uncoupled stream.
  const decoder = new OpusDecoder({
    channels: 1,
    streamCount: 1,
    coupledStreamCount: 0,
    channelMappingTable: [0],
  });
  await decoder.ready;
  let closed = false;
  return {
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
        const channel = channelData[0];
        if (channel && samplesDecoded > 0) {
          // Copy out of the decoder-owned buffer so the PCM can be transferred to the worklet.
          onPcm(channel.slice(0, samplesDecoded));
        }
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
