// What a scope face is pointed at, and what a gesture on it means. Separate from `ScopeFace.tsx`
// so that file exports only components: a module mixing the two costs Fast Refresh the component
// state it would otherwise preserve.
import { identify, suggestedAt } from "../../components/bandPlan";
import { formatMhz } from "../../components/format";
import { type SpectrumView, spanToOffset, viewToSpan } from "../../components/spectrumView";
import type { BandPlan, ChannelInfo, ChannelParams } from "../../lib/types";

/** Which instrument a scope draws. Both are wired the same way — a radio's IQ out, a channel's
 * baseband out — so a scope holding both wires shows one and offers the other. */
export type ScopeSource = "iq" | "baseband";

/**
 * The source a scope draws, given what the operator picked and what is wired into it.
 *
 * A pick the patch cannot honour falls back rather than blanking the face: pulling the IQ wire
 * out of a scope showing the radio leaves it on the channel tap it still has, and the toggle goes
 * with the wire. With nothing picked the tap wins, because a channel's passband is the narrower
 * answer — an operator who has run a tap into this scope is asking about that channel.
 */
export function scopeSource(chosen: ScopeSource, hasIq: boolean, hasTap: boolean): ScopeSource {
  if (chosen === "iq" && hasIq) {
    return "iq";
  }
  if (chosen === "baseband" && hasTap) {
    return "baseband";
  }
  return hasTap ? "baseband" : "iq";
}

/** A frequency the operator has pointed at: absolute, and as the offset from the receiver's
 * centre that a channel is tuned by. */
export interface ScopePick {
  hz: number;
  offsetHz: number;
}

/** The frequency under a screen fraction of the plot. */
export function pickAt(
  centerHz: number,
  spanHz: number,
  view: SpectrumView,
  at: number,
): ScopePick {
  const offsetHz = Math.round(spanToOffset(viewToSpan(view, at), spanHz));
  return { hz: centerHz + offsetHz, offsetHz };
}

/** What a bookmark saved from the plot opens with: the band plan's name for the frequency and the
 * mode it suggests there, so the ordinary case is one keystroke. */
export function bookmarkDraft(
  hz: number,
  plan: BandPlan | null,
): { label: string; mode: string | null } {
  const found = plan === null ? [] : identify(plan, hz);
  return {
    label: found[0]?.allocation.name ?? formatMhz(hz),
    mode: suggestedAt(found)?.type ?? null,
  };
}

/** The mode a channel drawn from the plot opens in: what the band plan allocates there, else
 * whatever the operator is already listening to on this scope, else the palette's own default. */
export function channelTypeAt(
  suggested: ChannelParams | null,
  listening: ChannelInfo | undefined,
): string {
  return suggested?.type ?? listening?.settings.params.type ?? "nfm";
}

/**
 * Channel nodes drawn at a frequency, waiting for the engine channel that apply will create.
 *
 * An offset is a channel *setting* and lives on the engine's channel — a channel node carries its
 * type and nothing else — so the gesture that draws the node cannot write the frequency with it.
 * It is recorded here and applied by the face on the state update that first binds the node.
 *
 * Module scope rather than a ref: switching between the patch and the rack remounts every face,
 * and a tune dropped by that switch would leave the new channel silently at the centre.
 */
const awaitingCreation = new Map<string, number>();

export function tuneOnCreate(node: string, offsetHz: number): void {
  awaitingCreation.set(node, offsetHz);
}

/** The offset `node` was drawn at, cleared as it is read: the opening tune lands once, and every
 * move after it is the operator's. */
export function takeCreationTune(node: string): number | undefined {
  const offsetHz = awaitingCreation.get(node);
  awaitingCreation.delete(node);
  return offsetHz;
}
