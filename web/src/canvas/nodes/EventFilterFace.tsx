import { kindLabel } from "../../components/decoderLog";
import { Chips, ChoiceChip, NumberChip, ToggleChip } from "../../components/face/Chips";
import { TextChip } from "../../components/face/TextChip";
import type { EventFilterNode, EventKindFacets, PatchNode, PatchNodeOf } from "../../lib/types";
import { eventSourcesOf, wiredSourcesOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import {
  FILTER_MODES,
  filterMode,
  filterSaid,
  formatIds,
  formatWords,
  fromTriState,
  kindsOffered,
  MAX_FILTER_DURATION_MS,
  type PredicateKey,
  parseIds,
  parseWords,
  sectionsFor,
  stationLabel,
  type TriState,
  toTriState,
} from "./eventFilter";
import { FaceBody, NodeShell } from "./NodeShell";

type TriKey = "has_position" | "encrypted" | "emergency";

const TRI_STATES = [
  { value: "any", label: "Either" },
  { value: "yes", label: "Only" },
  { value: "no", label: "Never" },
] as const;

export function EventFilterFace({ node }: { node: PatchNode }) {
  if (node.kind !== "event_filter") {
    return null;
  }
  return <Face node={node} />;
}

function Face({ node }: { node: PatchNodeOf<"event_filter"> }) {
  const workspace = useWorkspaceContext();
  const settings: EventFilterNode = node.data ?? {};
  const sources = eventSourcesOf(workspace.graph, node.id);
  const offered = kindsOffered(
    wiredSourcesOf(workspace.graph, node.id),
    workspace.context.channelTypes,
  );
  const facets = workspace.context.facets;
  const kinds = settings.kinds ?? [];
  const narrowed = kinds.length > 0 ? kinds : offered;
  const sections = sectionsFor(narrowed, facets);

  const edit = (next: Partial<EventFilterNode>) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "event_filter"
          ? { ...current, data: { ...current.data, ...next } }
          : current,
      ),
    }));
  };

  return (
    <NodeShell
      node={node}
      title="Event filter"
      category="tool"
      subtitle={sources.length > 0 ? filterSaid(settings) : undefined}
    >
      <FaceBody
        title={
          sources.length === 0
            ? "Wire decoder events in"
            : offered.length === 0
              ? "Nothing wired in emits events"
              : undefined
        }
      >
        <Chips className="p-2">
          <ChoiceChip
            label="Mode"
            title="Keep passes only what matches; Drop removes what matches"
            value={filterMode(settings)}
            options={FILTER_MODES}
            onChange={(mode) => edit({ mode })}
          />
          {offered.length > 1 &&
            offered.map((kind) => (
              <ToggleChip
                key={kind}
                label={kindLabel(kind)}
                title={`Match ${kindLabel(kind)} events`}
                on={kinds.includes(kind)}
                onChange={(on) =>
                  edit({
                    kinds: on ? [...kinds, kind].toSorted() : kinds.filter((held) => held !== kind),
                  })
                }
              />
            ))}
          {sections.flatMap((section) =>
            section.predicates.map((predicate) => (
              <Predicate
                key={predicate}
                which={predicate}
                applies={section.applies}
                settings={settings}
                kinds={narrowed}
                facets={facets}
                edit={edit}
              />
            )),
          )}
        </Chips>
      </FaceBody>
    </NodeShell>
  );
}

function appliesTo(title: string, applies: readonly string[]): string {
  return applies.length === 0 ? title : `${title}. Applies to ${applies.join(", ")}`;
}

function Predicate({
  which,
  applies,
  settings,
  kinds,
  facets,
  edit,
}: {
  which: PredicateKey;
  applies: readonly string[];
  settings: EventFilterNode;
  kinds: readonly string[];
  facets: readonly EventKindFacets[];
  edit: (next: Partial<EventFilterNode>) => void;
}) {
  const tri = (label: string, value: boolean | null | undefined, key: TriKey) => (
    <ChoiceChip
      label={label}
      title={appliesTo(label, applies)}
      value={toTriState(value)}
      options={TRI_STATES}
      quiet={value == null}
      onChange={(next: TriState) => edit({ [key]: fromTriState(next) ?? null })}
    />
  );
  switch (which) {
    case "stations": {
      const label = stationLabel(kinds, facets);
      return (
        <TextChip
          label={label}
          title={appliesTo(label, applies)}
          value={formatWords(settings.stations)}
          placeholder="any"
          onCommit={(text) => edit({ stations: parseWords(text) })}
        />
      );
    }
    case "contains":
      return (
        <TextChip
          label="Contains"
          title={appliesTo("Contains", applies)}
          value={settings.contains ?? ""}
          placeholder="any"
          onCommit={(text) => edit({ contains: text.trim() || null })}
        />
      );
    case "has_position":
      return tri("Has position", settings.has_position, "has_position");
    case "talkgroups":
      return (
        <TextChip
          label="Talkgroups"
          title={appliesTo("Talkgroups", applies)}
          value={formatIds(settings.talkgroups)}
          placeholder="any"
          onCommit={(text) => edit({ talkgroups: parseIds(text) })}
        />
      );
    case "radios":
      return (
        <TextChip
          label="Radios"
          title={appliesTo("Radios", applies)}
          value={formatIds(settings.radios)}
          placeholder="any"
          onCommit={(text) => edit({ radios: parseIds(text) })}
        />
      );
    case "encrypted":
      return tri("Encrypted", settings.encrypted, "encrypted");
    case "emergency":
      return tri("Emergency", settings.emergency, "emergency");
    case "min_duration_ms":
      return (
        <NumberChip
          label="Longer than"
          title={appliesTo("Longer than", applies)}
          value={(settings.min_duration_ms ?? 0) / 1000}
          unit="s"
          min={0}
          max={MAX_FILTER_DURATION_MS / 1000}
          step={0.5}
          quiet={(settings.min_duration_ms ?? 0) === 0}
          onCommit={(next) => edit({ min_duration_ms: Math.round(next * 1000) })}
        />
      );
  }
}
