export type DecoderRequest =
  | { type: "init"; channels: number }
  | { type: "reset" }
  | { type: "decode"; id: number; epoch: number; packet: Uint8Array };

export type DecoderResponse =
  | { type: "ready" }
  | { type: "pcm"; id: number; epoch: number; pcm: Float32Array }
  | { type: "error"; id?: number; epoch?: number; message: string };
