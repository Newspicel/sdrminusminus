import { identify, suggestedAt } from "../../components/bandPlan";
import { formatMhz } from "../../components/format";
import {
  type SpectrumView,
  spanToOffset,
  viewToSpan,
  viewWidth,
} from "../../components/spectrumView";
import type { BandPlan, ChannelInfo, ChannelParams } from "../../lib/types";

export function streamChannels(
  channels: readonly ChannelInfo[],
  stream: number,
): readonly ChannelInfo[] {
  return channels.filter((channel) => (channel.stream ?? 0) === stream);
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

export function dragTuneHz(
  centerHz: number,
  spanHz: number,
  view: SpectrumView,
  deltaPx: number,
  widthPx: number,
): number {
  if (!(spanHz > 0) || !(widthPx > 0)) {
    return Math.round(centerHz);
  }
  return Math.round(centerHz + (deltaPx / widthPx) * spanHz * viewWidth(view));
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

export function tuneOnCreate(node: string, frequencyHz: number): void {
  awaitingCreation.set(node, frequencyHz);
}

export function takeCreationTune(node: string): number | undefined {
  const frequencyHz = awaitingCreation.get(node);
  awaitingCreation.delete(node);
  return frequencyHz;
}
