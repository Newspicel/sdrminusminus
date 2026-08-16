import { describe, expect, it } from "vitest";
import type { ChannelDescriptor } from "../../lib/types";
import { keepsCalls } from "./callRecording";

const descriptor = (decoder_kind: string | null): ChannelDescriptor =>
  ({ type_id: "dmr", name: "DMR", decoder_kind }) as ChannelDescriptor;

describe("keepsCalls", () => {
  it("offers recording to every digital voice decoder", () => {
    expect(keepsCalls(descriptor("dv"))).toBe(true);
  });

  it("withholds it from decoders that carry no voice", () => {
    expect(keepsCalls(descriptor("aprs"))).toBe(false);
    expect(keepsCalls(descriptor(null))).toBe(false);
    expect(keepsCalls(undefined)).toBe(false);
  });
});
