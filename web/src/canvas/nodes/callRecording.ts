import type { ChannelDescriptor } from "../../lib/types";

export function keepsCalls(descriptor: ChannelDescriptor | undefined): boolean {
  return descriptor?.decoder_kind === "dv";
}
