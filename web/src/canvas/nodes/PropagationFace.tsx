import { useQueries, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { Chips, ChoiceChip, ToggleChip } from "../../components/face/Chips";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { formatMhz } from "../../components/format";
import { MapPanel } from "../../components/MapPanel";
import { decoderLogQuery, ionosondeQuery } from "../../lib/api";
import {
  type CellComparison,
  compareCells,
  forecastAgreement,
  forecastAt,
} from "../../lib/ionosonde";
import { mufColor, type PropagationLayer } from "../../lib/map/propagation";
import { positionSourcesOf, usePositionStore } from "../../lib/position";
import {
  EMPTY_SESSION,
  liveObservations,
  mergeObservations,
  observationOf,
  type PathObservation,
  PROPAGATION_KINDS,
  propagationCells,
  propagationPaths,
  propagationSummary,
  receiverOf,
  usePropagationStore,
} from "../../lib/propagation";
import type { DecodedRecord, PatchNode, PatchNodeOf, ServerEvent } from "../../lib/types";
import { type Input, inputsOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

const REDRAW_MS = 2_000;

const NO_POSITIONS: readonly string[] = [];

const NO_RECEIVER: readonly [number, number] = [0, 0];

const HISTORY_HOURS = 6;

const HISTORY_LIMIT = 2_000;

const EMPTY_SONDES: never[] = [];

const HALF_LIFE_OPTIONS = [
  { value: 5, label: "5 min" },
  { value: 15, label: "15 min" },
  { value: 30, label: "30 min" },
  { value: 60, label: "1 h" },
  { value: 120, label: "2 h" },
  { value: 360, label: "6 h" },
  { value: 720, label: "12 h" },
] as const;

const HEIGHT_OPTIONS = [
  { value: 110, label: "110 km (E)" },
  { value: 250, label: "250 km (F2 low)" },
  { value: 300, label: "300 km (F2)" },
  { value: 350, label: "350 km (F2 high)" },
  { value: 400, label: "400 km" },
] as const;

const LAYER_OPTIONS = [
  { value: "activity" as const, label: "Activity" },
  { value: "muf" as const, label: "MUF" },
];

export function PropagationFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const inputs = inputsOf(
    workspace.graph,
    node.id,
    "events",
    workspace.devices,
    workspace.channels,
    workspace.trunks,
    workspace.owners,
  );
  const wired = inputs.filter((input) =>
    (PROPAGATION_KINDS as readonly string[]).includes(
      workspace.context.channelTypes.find(
        (type) => type.type_id === input.channel.settings.params.type,
      )?.decoder_kind ?? "",
    ),
  );
  const positionNode = positionSourcesOf(workspace.graph, node.id)[0];
  const fix = usePositionStore((store) =>
    positionNode === undefined ? undefined : store.sources[positionNode]?.fix,
  );
  const receiver = receiverOf(fix ?? undefined);

  if (node.kind !== "propagation") {
    return null;
  }

  return (
    <NodeShell node={node} title="Propagation map" category="output">
      <Propagation
        node={node}
        inputs={wired}
        positionNode={positionNode ?? null}
        receiver={receiver}
      />
    </NodeShell>
  );
}

function Propagation({
  node,
  inputs,
  positionNode,
  receiver,
}: {
  node: PatchNodeOf<"propagation">;
  inputs: readonly Input[];
  positionNode: string | null;
  receiver: readonly [number, number] | null;
}) {
  const workspace = useWorkspaceContext();
  const active = useFaceActive();
  const session = usePropagationStore((store) => store.sessions[node.id]) ?? EMPTY_SESSION;
  const observe = usePropagationStore((store) => store.observe);
  const clear = usePropagationStore((store) => store.clear);
  const [layer, setLayer] = useState<PropagationLayer>("activity");
  const [clearArmed, setClearArmed] = useState(false);
  const [tick, setTick] = useState(() => Date.now());

  const settings = node.data;
  const heightKm = settings.reflection_height_km;
  const halfLifeMinutes = settings.half_life_minutes;
  const sources = inputs.map((input) => `${input.deviceSet}:${input.channel.id}`).join(",");
  const nodes = inputs.map((input) => input.node).join(",");
  const [latitude, longitude] = receiver ?? NO_RECEIVER;
  const station = useMemo<[number, number]>(() => [latitude, longitude], [latitude, longitude]);
  const options = useMemo(() => ({ halfLifeMinutes, nowMs: tick }), [halfLifeMinutes, tick]);

  useEffect(() => {
    const timer = setInterval(() => setTick(Date.now()), REDRAW_MS);
    return () => clearInterval(timer);
  }, []);

  const socket = workspace.socket;
  const located = receiver !== null;
  useEffect(() => {
    if (socket === null || !located) {
      return;
    }
    const wanted = new Set(sources === "" ? [] : sources.split(","));
    return socket.on("event", (event: ServerEvent) => {
      const records: DecodedRecord[] =
        event.type === "Decoded"
          ? [event.data]
          : event.type === "DecodedBacklog"
            ? event.data.records
            : [];
      const observations = records
        .filter((record) => wanted.has(`${record.device_set}:${record.channel}`))
        .map((record) => observationOf(record, station, heightKm))
        .filter((observation): observation is PathObservation => observation !== null);
      observe(node.id, observations);
    });
  }, [socket, located, sources, station, heightKm, node.id, observe]);

  const [since] = useState(() => new Date(Date.now() - HISTORY_HOURS * 3_600_000).toISOString());
  const stored = useQueries({
    queries: PROPAGATION_KINDS.map((kind) =>
      decoderLogQuery({ kind, nodes, sources, since, limit: HISTORY_LIMIT }),
    ),
    combine: (results) => results.flatMap((result) => result.data?.entries ?? []),
  });
  const seed = useMemo(
    () =>
      stored
        .map((entry) =>
          observationOf(
            {
              device_set: entry.device_set,
              channel: entry.channel,
              at: entry.at,
              freq_hz: entry.freq_hz,
              event: entry.event,
            },
            station,
            heightKm,
          ),
        )
        .filter((observation): observation is PathObservation => observation !== null),
    [stored, station, heightKm],
  );

  const held = useMemo(
    () => mergeObservations(seed, session.observations),
    [seed, session.observations],
  );
  const observations = useMemo(
    () => liveObservations(held, options, session.clearedAt),
    [held, session.clearedAt, options],
  );
  const cells = useMemo(() => propagationCells(observations, options), [observations, options]);
  const paths = useMemo(
    () => (settings.show_paths ? propagationPaths(observations, station, options) : EMPTY_PATHS),
    [observations, station, settings.show_paths, options],
  );
  const summary = useMemo(() => propagationSummary(observations), [observations]);

  const ionosonde = useQuery(ionosondeQuery(settings.compare_forecast));
  const reported = ionosonde.data?.stations;
  const sondes = useMemo(
    () => (settings.compare_forecast ? (reported ?? EMPTY_SONDES) : EMPTY_SONDES),
    [settings.compare_forecast, reported],
  );
  const comparisons = useMemo(() => compareCells(cells, sondes), [cells, sondes]);
  const agreement = useMemo(() => forecastAgreement(comparisons), [comparisons]);
  const overhead = useMemo(() => forecastAt(sondes, station[0], station[1]), [sondes, station]);

  const forecastError = ionosonde.data?.error ?? (ionosonde.isError ? "no answer" : null);

  const overlay = useMemo(() => ({ cells, paths, sondes, layer }), [cells, paths, sondes, layer]);

  const update = (patch: Partial<PatchNodeOf<"propagation">["data"]>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "propagation"
          ? { ...current, data: { ...current.data, ...patch } }
          : current,
      ),
    }));
  };

  return (
    <>
      <FaceBody scroll={false}>
        <MapPanel
          kinds={[]}
          positionNodes={positionNode === null ? NO_POSITIONS : [positionNode]}
          propagation={overlay}
          active={active}
          className="min-h-0 w-full flex-1"
        />
        <PropagationChips settings={settings} layer={layer} onLayer={setLayer} onChange={update} />
        <Readouts columns="fit">
          <Readout label="Decodes">{summary.decodes}</Readout>
          <Readout label="Grids">{summary.grids}</Readout>
          <Readout label="Cells">{cells.length}</Readout>
          <Readout label="Highest" title="Highest frequency heard">
            {formatMhz(summary.bestFreqHz)}
          </Readout>
          <Readout
            label="MUF(3000)"
            title="Measured MUF is a floor: the highest frequency decoded over each path, projected onto a 3000 km hop"
          >
            {summary.bestMuf3000Mhz === null ? "-" : `≥ ${summary.bestMuf3000Mhz.toFixed(1)} MHz`}
          </Readout>
          <Readout label="Farthest">{Math.round(summary.farthestKm)} km</Readout>
          {settings.compare_forecast && (
            <ForecastReadouts
              overheadMhz={overhead?.muf3000Mhz ?? null}
              source={ionosonde.data?.source ?? null}
              stations={sondes.length}
              above={agreement.above}
              cells={agreement.cells}
              medianDeltaMhz={agreement.medianDeltaMhz}
            />
          )}
        </Readouts>
        {settings.compare_forecast && forecastError !== null && (
          <FaceFault message="Ionosonde feed failed" detail={forecastError} />
        )}
        <div className="max-h-40 shrink-0 overflow-auto border-t border-line">
          <PathTable
            comparisons={comparisons}
            cells={cells}
            compareForecast={settings.compare_forecast}
          />
        </div>
      </FaceBody>
      <FaceFooter>
        <Button
          type="button"
          className={clearArmed ? BTN_DANGER : BTN}
          disabled={observations.length === 0}
          onBlur={() => setClearArmed(false)}
          onClick={() => {
            if (!clearArmed) {
              setClearArmed(true);
              return;
            }
            clear(node.id);
            setClearArmed(false);
          }}
        >
          {clearArmed ? "Confirm clear" : "Clear"}
        </Button>
      </FaceFooter>
    </>
  );
}

type PropagationSettings = PatchNodeOf<"propagation">["data"];

function PropagationChips({
  settings,
  layer,
  onLayer,
  onChange,
}: {
  settings: PropagationSettings;
  layer: PropagationLayer;
  onLayer: (layer: PropagationLayer) => void;
  onChange: (patch: Partial<PropagationSettings>) => void;
}) {
  return (
    <Chips className="shrink-0 border-t border-line p-2">
      <ChoiceChip
        label="Half-life"
        title="Decay half-life"
        value={settings.half_life_minutes}
        options={HALF_LIFE_OPTIONS}
        onChange={(half_life_minutes) => onChange({ half_life_minutes })}
      />
      <ChoiceChip
        label="Height"
        title="Reflecting layer height"
        value={settings.reflection_height_km}
        options={HEIGHT_OPTIONS}
        onChange={(reflection_height_km) => onChange({ reflection_height_km })}
      />
      <ChoiceChip
        label="Map"
        title="Map layer"
        value={layer}
        options={LAYER_OPTIONS}
        onChange={onLayer}
      />
      <ToggleChip
        label="Paths"
        title="Draw the path to every station heard"
        on={settings.show_paths}
        onChange={(show_paths) => onChange({ show_paths })}
      />
      <ToggleChip
        label="Ionosondes"
        title="Compare against the ionosonde network"
        on={settings.compare_forecast}
        onChange={(compare_forecast) => onChange({ compare_forecast })}
      />
    </Chips>
  );
}

function ForecastReadouts({
  overheadMhz,
  source,
  stations,
  above,
  cells,
  medianDeltaMhz,
}: {
  overheadMhz: number | null;
  source: string | null;
  stations: number;
  above: number;
  cells: number;
  medianDeltaMhz: number;
}) {
  const sign = medianDeltaMhz >= 0 ? "+" : "";
  return (
    <>
      <Readout label="Forecast" title="The ionosonde MUF(3000) over your location">
        {overheadMhz === null ? "-" : `${overheadMhz.toFixed(1)} MHz`}
      </Readout>
      <Readout label="Sites" title={`Sounding sites, from ${source ?? "the ionosonde network"}`}>
        {stations === 0 ? "waiting" : stations}
      </Readout>
      {stations > 0 && (
        <Readout label="Above" title="Compared cells that sit above the forecast">
          {above}/{cells}
        </Readout>
      )}
      {stations > 0 && (
        <Readout label="Median Δ" title="Median of measured minus forecast MUF">
          {sign}
          {medianDeltaMhz.toFixed(1)} MHz
        </Readout>
      )}
    </>
  );
}

const EMPTY_PATHS: never[] = [];

function PathTable({
  comparisons,
  cells,
  compareForecast,
}: {
  comparisons: readonly CellComparison[];
  cells: readonly ReturnType<typeof propagationCells>[number][];
  compareForecast: boolean;
}) {
  const rows = compareForecast
    ? comparisons.toSorted((a, b) => b.cell.weight - a.cell.weight).slice(0, 12)
    : [];
  const plain = compareForecast ? [] : cells.slice(0, 12);
  if (rows.length === 0 && plain.length === 0) {
    return (
      <p className="px-2 py-2 font-mono text-[10px] text-ink-faint">No reflection points yet.</p>
    );
  }
  return (
    <table className="w-full border-collapse">
      <thead className="sticky top-0 bg-panel-2">
        <tr>
          <th className={TABLE_HEAD}>Midpoint</th>
          <th className={TABLE_HEAD}>Decodes</th>
          <th className={TABLE_HEAD}>Highest</th>
          <th className={TABLE_HEAD}>Measured MUF</th>
          {compareForecast && <th className={TABLE_HEAD}>Forecast</th>}
          {compareForecast && <th className={TABLE_HEAD}>Δ</th>}
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.cell.key} className="border-t border-line">
            <td className={TABLE_CELL}>{row.cell.key}</td>
            <td className={TABLE_CELL}>{row.cell.decodes}</td>
            <td className={TABLE_CELL}>{formatMhz(row.cell.bestFreqHz)}</td>
            <td className={TABLE_CELL} style={{ color: mufColor(row.measuredMuf3000Mhz) }}>
              ≥ {row.measuredMuf3000Mhz.toFixed(1)}
            </td>
            <td className={TABLE_CELL}>{row.forecast.muf3000Mhz.toFixed(1)}</td>
            <td className={TABLE_CELL}>
              {row.deltaMhz >= 0 ? "+" : ""}
              {row.deltaMhz.toFixed(1)}
            </td>
          </tr>
        ))}
        {plain.map((cell) => (
          <tr key={cell.key} className="border-t border-line">
            <td className={TABLE_CELL}>{cell.key}</td>
            <td className={TABLE_CELL}>{cell.decodes}</td>
            <td className={TABLE_CELL}>{formatMhz(cell.bestFreqHz)}</td>
            <td className={TABLE_CELL}>
              {cell.measuredMuf3000Mhz === null ? "-" : `≥ ${cell.measuredMuf3000Mhz.toFixed(1)}`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
