import { Button } from "../../components/BaseControls";
import { BTN, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { useFusionClear, useFusionSeed, useFusionStore } from "../../lib/fusion";
import type { DfFusionState, DfStation, PatchNode, TriangulationParams } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FoldSection } from "./FoldSection";
import { FusionHeat } from "./FusionHeat";
import { FaceBody, FaceEmpty, FaceFooter, NodeShell } from "./NodeShell";
import { NO_CATALOG } from "./processorFace";
import {
  DECAY_OPTIONS,
  decayWith,
  estimateLabel,
  fadeTitle,
  fusionSources,
  guidanceText,
  MAX_HALF_LIFE_S,
  MIN_HALF_LIFE_S,
  NAV_OPTIONS,
  NO_SOURCES,
  spreadLabel,
  stationAge,
  stationBearing,
  stationSigma,
  triangulationSettings,
} from "./triangulation";

const AGE_TICK_MS = 1_000;
const STATION_HEADS = ["Station", "Last", "±", "Age"] as const;

type Edit = (next: Partial<TriangulationParams>) => void;

function useTriangulationEdit(id: string, base: TriangulationParams | null): Edit {
  const workspace = useWorkspaceContext();
  return (next) => {
    if (base === null) {
      return;
    }
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, id, (stored) =>
        stored.kind === "triangulation"
          ? {
              ...stored,
              data: { ...stored.data, settings: { ...(stored.data?.settings ?? base), ...next } },
            }
          : stored,
      ),
    }));
    workspace.apply();
  };
}

export function TriangulationFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const fusion = useFusionStore((store) => store.byNode[node.id]);
  useFusionSeed(node.id);
  const { clear, pending } = useFusionClear(node.id);
  const now = useNow(AGE_TICK_MS);
  const settings = triangulationSettings(node, workspace.context.catalog);
  const edit = useTriangulationEdit(node.id, settings);
  if (node.kind !== "triangulation") {
    return null;
  }
  const stations = fusion?.stations ?? [];
  const sources = fusionSources(workspace.graph, node.id);
  return (
    <NodeShell
      node={node}
      title="Triangulation"
      category="tool"
      subtitle={`${stations.length} of ${sources} reporting`}
    >
      <FaceBody>
        <FusionHeat
          node={node.id}
          estimate={fusion?.estimate ?? null}
          emitters={fusion?.emitters ?? []}
          stations={stations}
          hint={sources === 0 ? NO_SOURCES : null}
        />
        <FusionReadout fusion={fusion} />
        <StationTable stations={stations} now={now} />
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <TriangulationSettings settings={settings} fusion={fusion} edit={edit} />
        )}
      </FaceBody>
      <FaceFooter>
        <Button
          className={BTN}
          type="button"
          title="Throw away every bearing and start again"
          disabled={pending}
          onClick={clear}
        >
          Clear
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}

function FusionReadout({ fusion }: { fusion: DfFusionState | undefined }) {
  const estimate = fusion?.estimate ?? null;
  const guidance = guidanceText(fusion);
  const dropped = fusion?.dropped ?? 0;
  const refused = fusion?.refused ?? 0;
  return (
    <Readout>
      <ReadoutRow label="Estimate">{estimateLabel(estimate)}</ReadoutRow>
      <ReadoutRow label="Spread" title="One sigma error ellipse">
        {spreadLabel(estimate)}
      </ReadoutRow>
      <ReadoutRow label="Guidance" title={guidance.title}>
        {guidance.text}
      </ReadoutRow>
      <ReadoutRow label="Bearings">{fusion?.samples ?? 0}</ReadoutRow>
      {dropped > 0 && (
        <ReadoutRow label="Dropped" title="Bearings lost, the queue was full">
          <span className="text-danger">{dropped}</span>
        </ReadoutRow>
      )}
      {refused > 0 && (
        <ReadoutRow label="Refused" title="Bearings without a place or below Min conf">
          <span className="text-danger">{refused}</span>
        </ReadoutRow>
      )}
    </Readout>
  );
}

function StationTable({ stations, now }: { stations: readonly DfStation[]; now: number }) {
  if (stations.length === 0) {
    return null;
  }
  return (
    <table className="w-full shrink-0 border-t border-line" aria-label="Stations">
      <thead>
        <tr>
          {STATION_HEADS.map((head) => (
            <th key={head} className={TABLE_HEAD}>
              {head}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {stations.map((station) => (
          <tr key={station.station_id}>
            <td className={`${TABLE_CELL} max-w-24 truncate`} title={station.station_id}>
              {station.station_id}
            </td>
            <td className={TABLE_CELL}>{stationBearing(station)}</td>
            <td className={TABLE_CELL}>{stationSigma(station)}</td>
            <td className={TABLE_CELL}>{stationAge(station, now)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function TriangulationSettings({
  settings,
  fusion,
  edit,
}: {
  settings: TriangulationParams;
  fusion: DfFusionState | undefined;
  edit: Edit;
}) {
  const decay = settings.decay;
  return (
    <>
      <Settings className="border-t border-line p-2">
        <SettingRow label="Fade" title={fadeTitle(fusion)}>
          <Select
            label="Fade"
            value={decay.kind}
            options={DECAY_OPTIONS}
            onChange={(kind) => edit({ decay: decayWith(kind, decay) })}
          />
        </SettingRow>
        {decay.kind === "half_life" && (
          <SettingRow label="Half life" title="Old bearings count half after this">
            <NumberField
              label="Half life"
              unit="s"
              value={decay.seconds}
              min={MIN_HALF_LIFE_S}
              max={MAX_HALF_LIFE_S}
              step={10}
              onCommit={(seconds) =>
                edit({ decay: { kind: "half_life", seconds: Math.round(seconds) } })
              }
            />
          </SettingRow>
        )}
      </Settings>
      <FoldSection label="More">
        <MoreSettings settings={settings} edit={edit} />
      </FoldSection>
    </>
  );
}

function MoreSettings({ settings, edit }: { settings: TriangulationParams; edit: Edit }) {
  return (
    <Settings>
      <SettingRow label="Guide" title="Where to send a wired vehicle">
        <Segmented
          label="Guide"
          value={settings.nav}
          options={NAV_OPTIONS}
          onChange={(nav) => edit({ nav })}
        />
      </SettingRow>
      <SettingRow label="Extent" title="Half width of the search grid">
        <NumberField
          label="Extent"
          unit="km"
          value={settings.extent_km}
          min={1}
          max={100}
          step={0.1}
          onCommit={(extent_km) => edit({ extent_km })}
        />
      </SettingRow>
      <SettingRow label="Probe" title="How far across a single bearing to drive">
        <NumberField
          label="Probe"
          unit="km"
          value={settings.probe_km}
          min={0.5}
          max={50}
          step={0.5}
          onCommit={(probe_km) => edit({ probe_km })}
        />
      </SettingRow>
      <SettingRow label="Min conf" title="Bearings below this are refused">
        <NumberField
          label="Min conf"
          value={settings.min_confidence}
          min={0}
          max={1}
          step={0.01}
          onCommit={(min_confidence) => edit({ min_confidence })}
        />
      </SettingRow>
      <SettingRow label="Emitters" title="Most transmitters to find at once">
        <NumberField
          label="Emitters"
          value={settings.max_emitters}
          min={1}
          max={4}
          step={1}
          onCommit={(max_emitters) => edit({ max_emitters: Math.round(max_emitters) })}
        />
      </SettingRow>
    </Settings>
  );
}
