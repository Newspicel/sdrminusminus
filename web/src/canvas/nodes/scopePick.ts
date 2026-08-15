import { identify, suggestedAt } from "../../components/bandPlan";
import { formatMhz } from "../../components/format";
import { type SpectrumView, spanToOffset, viewToSpan } from "../../components/spectrumView";
import type { BandPlan, ChannelInfo, ChannelParams } from "../../lib/types";

export type ScopeSource = "iq" | "baseband";

export function scopeSource(chosen: ScopeSource, hasIq: boolean, hasTap: boolean): ScopeSource {
  if (chosen === "iq" && hasIq) {
    return "iq";
  }
  if (chosen === "baseband" && hasTap) {
    return "baseband";
  }
  return hasTap ? "baseband" : "iq";
}

export interface ScopePick {
  hz: number;
  offsetHz: number;
}

export function pickAt(
  centerHz: number,
  spanHz: number,
  view: SpectrumView,
  at: number,
): ScopePick {
  const offsetHz = Math.round(spanToOffset(viewToSpan(view, at), spanHz));
  return { hz: centerHz + offsetHz, offsetHz };
}

export function pickText(pick: ScopePick): { frequency: string; offset: string } {
  const offsetHz = Math.round(pick.offsetHz);
  return {
    frequency: `${Math.round(pick.hz)} Hz`,
    offset: `${offsetHz < 0 ? "-" : "+"}${Math.abs(offsetHz)} Hz`,
  };
}

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

export function channelTypeAt(
  suggested: ChannelParams | null,
  listening: ChannelInfo | undefined,
): string {
  return suggested?.type ?? listening?.settings.params.type ?? "nfm";
}

const awaitingCreation = new Map<string, number>();

export function tuneOnCreate(node: string, offsetHz: number): void {
  awaitingCreation.set(node, offsetHz);
}

export function takeCreationTune(node: string): number | undefined {
  const offsetHz = awaitingCreation.get(node);
  awaitingCreation.delete(node);
  return offsetHz;
}
