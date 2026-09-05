import { OpusDecoder } from "opus-decoder";
import type { DecoderRequest, DecoderResponse } from "./workerProtocol";

let decoder: OpusDecoder | null = null;

const report = (message: DecoderResponse, transfer: Transferable[] = []): void => {
  globalThis.postMessage(message, { transfer });
};

async function initialize(channels: number): Promise<void> {
  decoder?.free();
  decoder = new OpusDecoder({
    channels,
    streamCount: 1,
    coupledStreamCount: channels === 2 ? 1 : 0,
    channelMappingTable: channels === 2 ? [0, 1] : [0],
  });
  await decoder.ready;
  report({ type: "ready" });
}

function decode(message: Extract<DecoderRequest, { type: "decode" }>): void {
  try {
    if (decoder === null) {
      throw new Error("Opus decoder is not ready");
    }
    const { channelData, samplesDecoded, errors } = decoder.decodeFrame(message.packet);
    if (errors.length > 0 || samplesDecoded <= 0) {
      throw new Error(errors.map((error) => error.message).join("; ") || "Empty Opus frame");
    }
    const pcm = new Float32Array(samplesDecoded * channelData.length);
    for (let channel = 0; channel < channelData.length; channel++) {
      const plane = channelData[channel];
      if (!plane) {
        throw new Error("Missing Opus channel");
      }
      for (let frame = 0; frame < samplesDecoded; frame++) {
        pcm[frame * channelData.length + channel] = plane[frame] ?? 0;
      }
    }
    report({ type: "pcm", id: message.id, epoch: message.epoch, pcm }, [pcm.buffer]);
  } catch (error) {
    report({ type: "error", id: message.id, epoch: message.epoch, message: String(error) });
  }
}

let pending = Promise.resolve();
globalThis.onmessage = (event: MessageEvent<DecoderRequest>): void => {
  const message = event.data;
  pending = pending
    .then(async () => {
      if (message.type === "init") await initialize(message.channels);
      else if (message.type === "decode") decode(message);
      else await decoder?.reset();
    })
    .catch((error: unknown) => {
      report({ type: "error", message: String(error) });
    });
};
