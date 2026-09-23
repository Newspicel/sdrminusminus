import type { ChannelDescriptor, DecoderFamily } from "../../lib/types";
import { FAMILIES } from "../palette";

export interface Protocol {
  kind: string;
  label: string;
  name: string;
}

export interface ProtocolGroup {
  family: DecoderFamily;
  title: string;
  protocols: Protocol[];
}

export interface ProtocolChoice {
  disabled: readonly string[];
  unidentified: boolean;
}

export interface ProtocolPreset {
  id: string;
  label: string;
  choice: ProtocolChoice;
}

export function protocolGroups(types: readonly ChannelDescriptor[]): ProtocolGroup[] {
  return (Object.keys(FAMILIES) as DecoderFamily[])
    .map((family) => ({
      family,
      title: FAMILIES[family],
      protocols: types
        .filter((type) => type.identifiable === true && (type.family ?? "utility") === family)
        .map((type) => ({
          kind: type.type_id,
          label: type.name.replace(/\s*\(.*\)$/, ""),
          name: type.name,
        })),
    }))
    .filter((group) => group.protocols.length > 0);
}

function kindsOf(groups: readonly ProtocolGroup[]): string[] {
  return groups.flatMap((group) => group.protocols.map((protocol) => protocol.kind));
}

export function protocolPresets(groups: readonly ProtocolGroup[]): ProtocolPreset[] {
  const all = kindsOf(groups);
  return [
    { id: "all", label: "All", choice: { disabled: [], unidentified: true } },
    ...groups.map((group) => {
      const kept = new Set(kindsOf([group]));
      return {
        id: group.family,
        label: group.title,
        choice: { disabled: all.filter((kind) => !kept.has(kind)), unidentified: false },
      };
    }),
  ];
}

export function enabledKinds(
  groups: readonly ProtocolGroup[],
  disabled: readonly string[],
): string[] {
  const off = new Set(disabled);
  return kindsOf(groups).filter((kind) => !off.has(kind));
}

function sameChoice(
  groups: readonly ProtocolGroup[],
  a: ProtocolChoice,
  b: ProtocolChoice,
): boolean {
  const left = enabledKinds(groups, a.disabled);
  const right = enabledKinds(groups, b.disabled);
  return (
    a.unidentified === b.unidentified &&
    left.length === right.length &&
    left.every((kind, index) => kind === right[index])
  );
}

export function activePreset(
  groups: readonly ProtocolGroup[],
  choice: ProtocolChoice,
): ProtocolPreset | undefined {
  return protocolPresets(groups).find((preset) => sameChoice(groups, preset.choice, choice));
}

export function choiceSummary(groups: readonly ProtocolGroup[], choice: ProtocolChoice): string {
  const preset = activePreset(groups, choice);
  if (preset !== undefined) {
    return preset.id === "all" ? "All protocols" : preset.label;
  }
  const on = enabledKinds(groups, choice.disabled).length;
  if (on === 0) {
    return choice.unidentified ? "Unidentified only" : "Nothing";
  }
  return `${on} of ${kindsOf(groups).length}`;
}

export function setEnabled(
  disabled: readonly string[],
  kinds: readonly string[],
  enabled: boolean,
): string[] {
  const off = new Set(disabled);
  for (const kind of kinds) {
    if (enabled) {
      off.delete(kind);
    } else {
      off.add(kind);
    }
  }
  return [...off].toSorted();
}
