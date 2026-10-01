import { Button } from "../../components/BaseControls";
import { BTN, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { Chips, ChoiceChip, NumberChip } from "../../components/face/Chips";
import { Readout, Readouts } from "../../components/face/Readouts";
import { FaceStats, Stat } from "../../components/face/Stats";
import { formatCount } from "../../components/format";
import { useFusionClear, useFusionSeed, useFusionStore } from "../../lib/fusion";
import { FUSION_LIMITS } from "../../lib/limits";
import type { DfFusionState, DfStation, PatchNode, TriangulationParams } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
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
          known={fusion !== undefined}
          estimate={fusion?.estimate ?? null}
          emitters={fusion?.emitters ?? []}
          stations={stations}
          hint={sources === 0 ? NO_SOURCES : null}
        />
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <TriangulationChips settings={settings} fusion={fusion} edit={edit} />
        )}
        <FusionReadout fusion={fusion} />
        <StationTable stations={stations} now={now} />
      </FaceBody>
      <FaceFooter>
        <FusionStats fusion={fusion} />
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
  return (
    <Readouts>
      <Readout label="Estimate">{estimateLabel(estimate)}</Readout>
      <Readout label="Spread" title="One sigma error ellipse">
        {spreadLabel(estimate)}
      </Readout>
      <Readout label="Guidance" title={guidance.title}>
        {guidance.text}
      </Readout>
      <Readout label="Bearings">{fusion?.samples ?? 0}</Readout>
    </Readouts>
  );
}

function FusionStats({ fusion }: { fusion: DfFusionState | undefined }) {
  const dropped = fusion?.dropped ?? 0;
  const refused = fusion?.refused ?? 0;
  return (
    <FaceStats>
      {dropped > 0 && (
        <Stat label="Dropped" title="Bearings lost, the queue was full" tone="danger">
          {formatCount(dropped)}
        </Stat>
      )}
      {refused > 0 && (
        <Stat label="Refused" title="Bearings without a place or below Min conf" tone="danger">
          {formatCount(refused)}
        </Stat>
      )}
    </FaceStats>
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

function TriangulationChips({
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
    <Chips className="p-2">
      <ChoiceChip
        label="Fade"
        title={fadeTitle(fusion) ?? "How fast old bearings count less"}
        value={decay.kind}
        options={DECAY_OPTIONS}
        onChange={(kind) => edit({ decay: decayWith(kind, decay) })}
      />
      {decay.kind === "half_life" && (
        <NumberChip
          label="Half life"
          title="Old bearings count half after this"
          unit="s"
          value={decay.seconds}
          min={FUSION_LIMITS.half_life_s.min}
          max={FUSION_LIMITS.half_life_s.max}
          step={10}
          onCommit={(seconds) =>
            edit({ decay: { kind: "half_life", seconds: Math.round(seconds) } })
          }
        />
      )}
      <ChoiceChip
        label="Guide"
        title="Where to send a wired vehicle"
        value={settings.nav}
        options={NAV_OPTIONS}
        quiet={settings.nav === "off"}
        onChange={(nav) => edit({ nav })}
      />
      <NumberChip
        label="Extent"
        title="Half width of the search grid"
        unit="km"
        value={settings.extent_km}
        min={FUSION_LIMITS.extent_km.min}
        max={FUSION_LIMITS.extent_km.max}
        step={0.1}
        onCommit={(extent_km) => edit({ extent_km })}
      />
      <NumberChip
        label="Probe"
        title="How far across a single bearing to drive"
        unit="km"
        value={settings.probe_km}
        min={FUSION_LIMITS.probe_km.min}
        max={FUSION_LIMITS.probe_km.max}
        step={0.5}
        onCommit={(probe_km) => edit({ probe_km })}
      />
      <NumberChip
        label="Min conf"
        title="Bearings below this are refused"
        value={settings.min_confidence}
        min={FUSION_LIMITS.min_confidence.min}
        max={FUSION_LIMITS.min_confidence.max}
        step={0.01}
        onCommit={(min_confidence) => edit({ min_confidence })}
      />
      <NumberChip
        label="Emitters"
        title="Most transmitters to find at once"
        value={settings.max_emitters}
        min={FUSION_LIMITS.emitters.min}
        max={FUSION_LIMITS.emitters.max}
        step={1}
        onCommit={(max_emitters) => edit({ max_emitters: Math.round(max_emitters) })}
      />
    </Chips>
  );
}
