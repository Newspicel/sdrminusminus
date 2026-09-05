import type { DecoderRequest, DecoderResponse } from "./workerProtocol";
import { SAMPLE_RATE } from "./worklet";

export interface OpusPacketDecoder {
  readonly channels: number;
  decode(packet: Uint8Array, timestampUs: number): boolean;
  reset(): void;
  close(): void;
}

const MAX_DECODE_QUEUE = 5;
const MAX_DECODE_AGE_MS = 150;

function config(channels: number): AudioDecoderConfig {
  return { codec: "opus", sampleRate: SAMPLE_RATE, numberOfChannels: channels };
}

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
  onDropped: (frames: number) => void = () => {},
): Promise<OpusPacketDecoder> {
  if (await webCodecsSupported(channels)) {
    return createWebCodecsDecoder(channels, onPcm, onError, onDropped);
  }
  return createWasmDecoder(channels, onPcm, onError, onDropped);
}

function createWebCodecsDecoder(
  channels: number,
  onPcm: (pcm: Float32Array) => void,
  onError: (err: unknown) => void,
  onDropped: (frames: number) => void,
): OpusPacketDecoder {
  const pending = new Map<number, number>();
  const decoder = new AudioDecoder({
    output: (data) => {
      try {
        const started = pending.get(data.timestamp);
        pending.delete(data.timestamp);
        if (started === undefined || performance.now() - started > MAX_DECODE_AGE_MS) {
          onDropped(data.numberOfFrames);
          return;
        }
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
      if (decoder.state !== "configured" || pending.size >= MAX_DECODE_QUEUE) {
        return false;
      }
      pending.set(timestampUs, performance.now());
      decoder.decode(new EncodedAudioChunk({ type: "key", timestamp: timestampUs, data: packet }));
      return true;
    },
    reset() {
      if (decoder.state !== "closed") {
        pending.clear();
        decoder.reset();
        decoder.configure(config(channels));
      }
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
  onDropped: (frames: number) => void,
): Promise<OpusPacketDecoder> {
  const worker = new Worker(new URL("./opusWorker.ts", import.meta.url), { type: "module" });
  const pending = new Map<number, number>();
  let closed = false;
  let epoch = 0;
  let sequence = 0;
  const post = (message: DecoderRequest, transfer: Transferable[] = []): void => {
    worker.postMessage(message, transfer);
  };
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => {
      worker.terminate();
      reject(new Error("Opus decoder initialization timed out"));
    }, 10_000);
    worker.onerror = (event) => {
      clearTimeout(timer);
      worker.terminate();
      closed = true;
      reject(new Error(event.message));
      onError(new Error(event.message));
    };
    worker.onmessage = (event: MessageEvent<DecoderResponse>) => {
      const message = event.data;
      if (message.type === "ready") {
        clearTimeout(timer);
        resolve();
        return;
      }
      if (closed) {
        return;
      }
      if (message.type === "error" && message.id === undefined) {
        clearTimeout(timer);
        worker.terminate();
        closed = true;
        reject(new Error(message.message));
        return;
      }
      const started = message.id === undefined ? undefined : pending.get(message.id);
      if (message.id !== undefined) {
        pending.delete(message.id);
      }
      if (message.epoch !== epoch) {
        return;
      }
      if (message.type === "error") {
        onError(new Error(message.message));
      } else if (started !== undefined) {
        if (performance.now() - started <= MAX_DECODE_AGE_MS) {
          onPcm(message.pcm);
        } else {
          onDropped(message.pcm.length / channels);
        }
      }
    };
    post({ type: "init", channels });
  });
  return {
    channels,
    decode(packet) {
      if (closed || pending.size >= MAX_DECODE_QUEUE) {
        return false;
      }
      const id = sequence++;
      const owned = packet.slice();
      pending.set(id, performance.now());
      post({ type: "decode", id, epoch, packet: owned }, [owned.buffer]);
      return true;
    },
    reset() {
      epoch++;
      post({ type: "reset" });
    },
    close() {
      closed = true;
      pending.clear();
      worker.terminate();
    },
  };
}
