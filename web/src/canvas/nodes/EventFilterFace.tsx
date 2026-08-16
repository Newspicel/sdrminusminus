import { Input } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { CHIP, FIELD, LABEL } from "../../components/controls";
import { kindLabel } from "../../components/decoderLog";
import { NumberField } from "../../components/NumberField";
import { Select } from "../../components/Select";
import { SettingGroup, SettingRow, Settings } from "../../components/Settings";
import type { EventFilterNode, PatchNode, PatchNodeOf } from "../../lib/types";
import { eventSourcesOf, targetsOf, wiredSourcesOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import {
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
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

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
  const targets = targetsOf(workspace.graph, node.id, "events");
  const offered = kindsOffered(
    wiredSourcesOf(workspace.graph, node.id),
    workspace.context.channelTypes,
  );
  const kinds = settings.kinds ?? [];
  const narrowed = kinds.length > 0 ? kinds : offered;
  const sections = sectionsFor(narrowed);

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
      category="feature"
      subtitle={sources.length > 0 ? filterSaid(settings) : undefined}
      live={sources.length > 0 && targets.length > 0}
    >
      <FaceBody>
        {sources.length === 0 ? (
          <FaceEmpty>Wire decoder events into the events input.</FaceEmpty>
        ) : offered.length === 0 ? (
          <FaceEmpty>Nothing wired in emits events.</FaceEmpty>
        ) : (
          <>
            {offered.length > 1 && (
              <div className="flex flex-col gap-1.5 border-b border-line p-2">
                <span className={LABEL}>Kinds</span>
                <ul className="flex flex-wrap gap-1">
                  {offered.map((kind) => (
                    <li key={kind}>
                      <label className={`${CHIP} cursor-pointer gap-1.5`}>
                        <Checkbox
                          label={kindLabel(kind)}
                          checked={kinds.includes(kind)}
                          onChange={(on) =>
                            edit({
                              kinds: on
                                ? [...kinds, kind].toSorted()
                                : kinds.filter((held) => held !== kind),
                            })
                          }
                        />
                        {kindLabel(kind)}
                      </label>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            <Settings className="p-2">
              {sections.map((section) => (
                <SettingGroup
                  key={section.key}
                  label={
                    <>
                      {section.title}
                      {section.applies.length > 0 && (
                        <span className="font-normal normal-case tracking-normal text-ink-faint">
                          {section.applies.join(" · ")}
                        </span>
                      )}
                    </>
                  }
                >
                  {section.predicates.map((predicate) => (
                    <Predicate
                      key={predicate}
                      which={predicate}
                      settings={settings}
                      kinds={narrowed}
                      edit={edit}
                    />
                  ))}
                </SettingGroup>
              ))}
            </Settings>
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function Predicate({
  which,
  settings,
  kinds,
  edit,
}: {
  which: PredicateKey;
  settings: EventFilterNode;
  kinds: readonly string[];
  edit: (next: Partial<EventFilterNode>) => void;
}) {
  switch (which) {
    case "stations": {
      const label = stationLabel(kinds);
      return (
        <SettingRow label={label}>
          <Input
            aria-label={label}
            className={FIELD}
            defaultValue={formatWords(settings.stations)}
            placeholder="any"
            onBlur={(event) => edit({ stations: parseWords(event.target.value) })}
          />
        </SettingRow>
      );
    }
    case "contains":
      return (
        <SettingRow label="Contains">
          <Input
            aria-label="Contains"
            className={FIELD}
            defaultValue={settings.contains ?? ""}
            placeholder="any text"
            onBlur={(event) => edit({ contains: event.target.value.trim() || null })}
          />
        </SettingRow>
      );
    case "has_position":
      return (
        <SettingRow label="Has position">
          <Select
            label="Has position"
            value={toTriState(settings.has_position)}
            options={TRI_STATES}
            onChange={(next: TriState) => edit({ has_position: fromTriState(next) ?? null })}
          />
        </SettingRow>
      );
    case "talkgroups":
      return (
        <SettingRow label="Talkgroups">
          <Input
            aria-label="Talkgroups"
            className={FIELD}
            defaultValue={formatIds(settings.talkgroups)}
            placeholder="any"
            onBlur={(event) => edit({ talkgroups: parseIds(event.target.value) })}
          />
        </SettingRow>
      );
    case "radios":
      return (
        <SettingRow label="Radios">
          <Input
            aria-label="Radios"
            className={FIELD}
            defaultValue={formatIds(settings.radios)}
            placeholder="any"
            onBlur={(event) => edit({ radios: parseIds(event.target.value) })}
          />
        </SettingRow>
      );
    case "encrypted":
      return (
        <SettingRow label="Encrypted">
          <Select
            label="Encrypted"
            value={toTriState(settings.encrypted)}
            options={TRI_STATES}
            onChange={(next: TriState) => edit({ encrypted: fromTriState(next) ?? null })}
          />
        </SettingRow>
      );
    case "emergency":
      return (
        <SettingRow label="Emergency">
          <Select
            label="Emergency"
            value={toTriState(settings.emergency)}
            options={TRI_STATES}
            onChange={(next: TriState) => edit({ emergency: fromTriState(next) ?? null })}
          />
        </SettingRow>
      );
    case "min_duration_ms":
      return (
        <SettingRow label="Longer than">
          <NumberField
            label="Longer than"
            value={(settings.min_duration_ms ?? 0) / 1000}
            min={0}
            max={MAX_FILTER_DURATION_MS / 1000}
            step={0.5}
            onCommit={(next) => edit({ min_duration_ms: Math.round(next * 1000) })}
          />
        </SettingRow>
      );
  }
}
