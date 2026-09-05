import { describe, expect, it } from "vitest";
import fixtures from "../generated/frame-fixtures.json";
import {
  decodeAudio,
  decodeIq,
  decodeRangeDoppler,
  decodeSpectrum,
  decodeSymbols,
  decodeVideo,
} from "./frame";

const decoders = {
  spectrum: decodeSpectrum,
  audio: decodeAudio,
  iq: decodeIq,
  symbols: decodeSymbols,
  surface: decodeRangeDoppler,
  gray: decodeVideo,
  rgb: decodeVideo,
};

describe("Rust binary frames", () => {
  for (const [name, values] of fixtures as [keyof typeof decoders, number[]][]) {
    it(`decodes the actual ${name} encoder output`, () => {
      const buffer = new Uint8Array(values).buffer;
      const frame = decoders[name](buffer);
      expect(frame).toMatchObject({ streamId: 1, seq: 2, timestamp: 3n });
      if (name === "iq") expect(decodeIq(buffer)?.samples).toEqual(new Float32Array([0.25, -0.5]));
      if (name === "spectrum")
        expect(decodeSpectrum(buffer)?.bins).toEqual(new Uint8Array([0, 127, 255]));
      if (name === "symbols")
        expect(decodeSymbols(buffer)?.symbols).toEqual(new Float32Array([0.5, -0.5]));
      for (let length = 0; length < 16; length++)
        expect(decoders[name](buffer.slice(0, length))).toBeNull();
    });
  }
  it("rejects truncated IQ components and declared payloads", () => {
    for (const [name, values] of fixtures as [keyof typeof decoders, number[]][]) {
      if (name === "audio") continue;
      expect(decoders[name](new Uint8Array(values.slice(0, -1)).buffer)).toBeNull();
    }
  });
});
