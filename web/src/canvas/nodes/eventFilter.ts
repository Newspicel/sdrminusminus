import { eventStation, eventSummary, hasPosition } from "../../components/eventFacts";
import type { ChannelDescriptor, DecoderEvent, EventFilterNode } from "../../lib/types";

export const MAX_FILTER_IDS = 256;
export const MAX_FILTER_DURATION_MS = 600_000;
export const MAX_FILTER_TEXT_LEN = 128;

export type TriState = "any" | "yes" | "no";

export function parseIds(text: string): number[] {
  const seen = new Set<number>();
  for (const token of text.split(/[\s,]+/)) {
    if (token === "") {
      continue;
    }
    const value = Number(token);
    if (!Number.isInteger(value) || value < 0) {
      continue;
    }
    seen.add(value);
  }
  return [...seen].slice(0, MAX_FILTER_IDS);
}

export function formatIds(ids: readonly number[] | undefined): string {
  return (ids ?? []).join(", ");
}

export function parseWords(text: string): string[] {
  const seen = new Set<string>();
  for (const token of text.split(/[\s,]+/)) {
    const word = token.trim();
    if (word !== "" && word.length <= MAX_FILTER_TEXT_LEN) {
      seen.add(word);
    }
  }
  return [...seen].slice(0, MAX_FILTER_IDS);
}

export function formatWords(words: readonly string[] | undefined): string {
  return (words ?? []).join(", ");
}

export function stationLabel(kinds: readonly string[]): string {
  if (kinds.length > 0 && kinds.every((kind) => kind === "adsb")) {
    return "Aircraft";
  }
  if (kinds.length > 0 && kinds.every((kind) => kind === "ais")) {
    return "Vessels";
  }
  if (kinds.length > 0 && kinds.every((kind) => VOICE_KINDS.includes(kind))) {
    return "Radios seen";
  }
  return "Stations";
}

export function toTriState(value: boolean | null | undefined): TriState {
  if (value == null) {
    return "any";
  }
  return value ? "yes" : "no";
}

export function fromTriState(state: TriState): boolean | undefined {
  if (state === "any") {
    return undefined;
  }
  return state === "yes";
}

export interface WiredSource {
  channelType?: string;
  recordsCalls: boolean;
  trunk: boolean;
}

export function kindsOffered(
  sources: readonly WiredSource[],
  descriptors: readonly ChannelDescriptor[],
): string[] {
  const kinds = new Set<string>();
  for (const source of sources) {
    if (source.trunk) {
      kinds.add("dv");
    }
    const kind = descriptors.find((d) => d.type_id === source.channelType)?.decoder_kind;
    if (kind != null) {
      kinds.add(kind);
    }
    if (source.recordsCalls) {
      kinds.add("call");
    }
  }
  return [...kinds].toSorted();
}

export function filterSaid(filter: EventFilterNode): string {
  const parts: string[] = [];
  const kinds = filter.kinds ?? [];
  parts.push(kinds.length === 0 ? "every event" : kinds.join(", "));
  if ((filter.stations ?? []).length > 0) {
    parts.push(formatWords(filter.stations));
  }
  if ((filter.contains ?? "") !== "") {
    parts.push(`"${filter.contains}"`);
  }
  if (filter.has_position != null) {
    parts.push(filter.has_position ? "with a fix" : "without a fix");
  }
  if ((filter.talkgroups ?? []).length > 0) {
    parts.push(`TG ${formatIds(filter.talkgroups)}`);
  }
  if ((filter.radios ?? []).length > 0) {
    parts.push(`radio ${formatIds(filter.radios)}`);
  }
  if (filter.encrypted != null) {
    parts.push(filter.encrypted ? "encrypted" : "clear");
  }
  if (filter.emergency != null) {
    parts.push(filter.emergency ? "emergency" : "routine");
  }
  if ((filter.min_duration_ms ?? 0) > 0) {
    parts.push(`over ${((filter.min_duration_ms ?? 0) / 1000).toFixed(1)} s`);
  }
  return parts.join(" · ");
}

export const VOICE_KINDS = ["call", "dv"];

export const POSITION_KINDS = [
  "adsb",
  "ais",
  "aprs",
  "dv",
  "dsc",
  "inmarsat_stdc",
  "inmarsat_aero",
  "vdl2",
  "hfdl",
  "iridium",
];

export const DURATION_KINDS = ["call"];

export type PredicateKey =
  | "stations"
  | "contains"
  | "has_position"
  | "talkgroups"
  | "radios"
  | "encrypted"
  | "emergency"
  | "min_duration_ms";

export function predicatesFor(kinds: readonly string[]): PredicateKey[] {
  const touches = (applies: readonly string[]) =>
    kinds.length === 0 || kinds.some((kind) => applies.includes(kind));
  const shown: PredicateKey[] = ["stations", "contains"];
  if (touches(POSITION_KINDS)) {
    shown.push("has_position");
  }
  if (touches(VOICE_KINDS)) {
    shown.push("talkgroups", "radios", "encrypted", "emergency");
  }
  if (touches(DURATION_KINDS)) {
    shown.push("min_duration_ms");
  }
  return shown;
}

export interface PredicateSection {
  key: string;
  title: string;
  applies: string[];
  predicates: PredicateKey[];
}

export function sectionsFor(kinds: readonly string[]): PredicateSection[] {
  const shown = predicatesFor(kinds);
  const pick = (keys: PredicateKey[]) => keys.filter((key) => shown.includes(key));
  const scope = (applies: readonly string[]) =>
    kinds.filter((kind) => applies.includes(kind)).toSorted();
  const sections: PredicateSection[] = [];
  const any = pick(["stations", "contains"]);
  if (any.length > 0) {
    sections.push({
      key: "any",
      title: "Any event",
      applies: [...kinds].toSorted(),
      predicates: any,
    });
  }
  const position = pick(["has_position"]);
  if (position.length > 0) {
    sections.push({
      key: "position",
      title: "Position",
      applies: scope(POSITION_KINDS),
      predicates: position,
    });
  }
  const voice = pick(["talkgroups", "radios", "encrypted", "emergency", "min_duration_ms"]);
  if (voice.length > 0) {
    sections.push({
      key: "voice",
      title: "Voice",
      applies: scope(VOICE_KINDS),
      predicates: voice,
    });
  }
  return sections;
}

interface Voice {
  source?: number | null;
  destination?: number | null;
  encrypted?: boolean | null;
  emergency?: boolean | null;
  duration_ms?: number;
}

function voiceOf(event: DecoderEvent): Voice | null {
  if (event.kind === "call") {
    return {
      source: event.data.source,
      destination: event.data.destination,
      encrypted: event.data.encrypted,
      emergency: event.data.emergency,
      duration_ms: event.data.duration_ms,
    };
  }
  if (event.kind === "dv") {
    return {
      source: event.data.source,
      destination: event.data.destination,
      encrypted: event.data.encrypted,
      emergency: event.data.emergency,
    };
  }
  return null;
}

export function passesFilter(filter: EventFilterNode, event: DecoderEvent): boolean {
  const kinds = filter.kinds ?? [];
  if (kinds.length > 0 && !kinds.includes(event.kind)) {
    return false;
  }
  const stations = filter.stations ?? [];
  if (stations.length > 0) {
    const station = eventStation(event);
    if (
      station === null ||
      !stations.some((want) => want.toLowerCase() === station.toLowerCase())
    ) {
      return false;
    }
  }
  const contains = filter.contains ?? "";
  if (contains !== "" && !eventSummary(event).toLowerCase().includes(contains.toLowerCase())) {
    return false;
  }
  if (filter.has_position != null && filter.has_position !== hasPosition(event)) {
    return false;
  }
  const voice = voiceOf(event);
  if (voice === null) {
    return true;
  }
  const talkgroups = filter.talkgroups ?? [];
  if (
    talkgroups.length > 0 &&
    (voice.destination == null || !talkgroups.includes(voice.destination))
  ) {
    return false;
  }
  const radios = filter.radios ?? [];
  if (radios.length > 0 && (voice.source == null || !radios.includes(voice.source))) {
    return false;
  }
  if (filter.encrypted != null && filter.encrypted !== voice.encrypted) {
    return false;
  }
  if (filter.emergency != null && filter.emergency !== voice.emergency) {
    return false;
  }
  return voice.duration_ms == null || voice.duration_ms >= (filter.min_duration_ms ?? 0);
}

export function passesChain(filters: readonly EventFilterNode[], event: DecoderEvent): boolean {
  return filters.every((filter) => passesFilter(filter, event));
}
