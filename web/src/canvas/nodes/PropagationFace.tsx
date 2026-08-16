import { useQueries, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Button } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { BTN, BTN_DANGER, LABEL, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { formatMhz } from "../../components/format";
import { MapPanel } from "../../components/MapPanel";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
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
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";

const REDRAW_MS = 2_000;

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
    <NodeShell
      node={node}
      title="Propagation map"
      category="display"
      subtitle={wired.length > 0 ? `${wired.length} in` : undefined}
      live={wired.length > 0 && receiver !== null}
    >
      <FaceBody scroll={false}>
        {wired.length === 0 ? (
          <FaceEmpty>
            Wire an FT8, FT4 or WSPR decoder's events in. Those carry the transmitting station's
            grid square, which is what a path is drawn from.
          </FaceEmpty>
        ) : positionNode === undefined || receiver === null ? (
          <FaceEmpty>
            {positionNode === undefined
              ? "Wire a GPS position in. Every path is measured from where this receiver stands, so the map needs that end of it."
              : "Waiting for a position fix."}
          </FaceEmpty>
        ) : (
          <Propagation node={node} inputs={wired} positionNode={positionNode} receiver={receiver} />
        )}
      </FaceBody>
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
  positionNode: string;
  receiver: readonly [number, number];
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
  const [latitude, longitude] = receiver;
  const station = useMemo<[number, number]>(() => [latitude, longitude], [latitude, longitude]);
  const options = useMemo(() => ({ halfLifeMinutes, nowMs: tick }), [halfLifeMinutes, tick]);

  useEffect(() => {
    const timer = setInterval(() => setTick(Date.now()), REDRAW_MS);
    return () => clearInterval(timer);
  }, []);

  const socket = workspace.socket;
  useEffect(() => {
    if (socket === null) {
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
  }, [socket, sources, station, heightKm, node.id, observe]);

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
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-line bg-panel-2 p-2">
        <label className={`${LABEL} flex flex-col items-stretch gap-1`}>
          Half-life
          <Select
            label="Decay half-life"
            className="w-28"
            value={settings.half_life_minutes}
            options={HALF_LIFE_OPTIONS}
            onChange={(half_life_minutes) => update({ half_life_minutes })}
          />
        </label>
        <label className={`${LABEL} flex flex-col items-stretch gap-1`}>
          Reflection
          <Select
            label="Reflecting layer height"
            className="w-36"
            value={settings.reflection_height_km}
            options={HEIGHT_OPTIONS}
            onChange={(reflection_height_km) => update({ reflection_height_km })}
          />
        </label>
        <Segmented label="Map layer" value={layer} options={LAYER_OPTIONS} onChange={setLayer} />
        <label className={`${LABEL} gap-1.5`}>
          <Checkbox
            label="Draw the path to every station heard"
            checked={settings.show_paths}
            onChange={(show_paths) => update({ show_paths })}
          />
          Paths
        </label>
        <label className={`${LABEL} gap-1.5`}>
          <Checkbox
            label="Compare against the ionosonde network"
            checked={settings.compare_forecast}
            onChange={(compare_forecast) => update({ compare_forecast })}
          />
          Ionosondes
        </label>
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
      </div>

      <div className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-b border-line px-2 py-1 font-mono text-[10px] tabular-nums text-ink-dim">
        <span>
          <span className="text-ink">{summary.decodes}</span> decodes
        </span>
        <span>
          <span className="text-ink">{summary.grids}</span> grids
        </span>
        <span>
          <span className="text-ink">{cells.length}</span> cells
        </span>
        <span>
          highest heard <span className="text-ink">{formatMhz(summary.bestFreqHz)}</span>
        </span>
        <span title="The highest frequency you actually decoded, projected onto a 3000 km hop. It is a floor: the real MUF sits at or above it.">
          measured MUF(3000){" "}
          <span className="text-ink">
            {summary.bestMuf3000Mhz === null ? "—" : `≥ ${summary.bestMuf3000Mhz.toFixed(1)} MHz`}
          </span>
        </span>
        <span>
          farthest <span className="text-ink">{Math.round(summary.farthestKm)} km</span>
        </span>
        {settings.compare_forecast && (
          <span title="The ionosonde network's MUF(3000) interpolated over your own location.">
            overhead forecast{" "}
            <span className="text-ink">
              {overhead === null ? "—" : `${overhead.muf3000Mhz.toFixed(1)} MHz`}
            </span>
          </span>
        )}
      </div>

      <MapPanel
        kinds={[]}
        positionNodes={[positionNode]}
        propagation={overlay}
        active={active}
        className="min-h-0 w-full flex-1"
      />

      <div className="max-h-40 shrink-0 overflow-auto border-t border-line">
        <PathTable
          comparisons={comparisons}
          cells={cells}
          compareForecast={settings.compare_forecast}
        />
      </div>

      <div className="shrink-0 border-t border-line px-2 py-1 font-mono text-[10px] text-ink-faint">
        {settings.compare_forecast ? (
          <ForecastNotice
            error={ionosonde.data?.error ?? (ionosonde.isError ? "no answer" : null)}
            source={ionosonde.data?.source ?? null}
            stations={sondes.length}
            above={agreement.above}
            cells={agreement.cells}
            medianDeltaMhz={agreement.medianDeltaMhz}
          />
        ) : (
          "Measured MUF is a floor — the highest frequency actually decoded over each path, projected onto a 3000 km hop."
        )}
      </div>
    </div>
  );
}

const EMPTY_PATHS: never[] = [];

function ForecastNotice({
  error,
  source,
  stations,
  above,
  cells,
  medianDeltaMhz,
}: {
  error: string | null;
  source: string | null;
  stations: number;
  above: number;
  cells: number;
  medianDeltaMhz: number;
}) {
  if (error !== null) {
    return <span className="text-danger">Ionosonde feed: {error}</span>;
  }
  if (stations === 0) {
    return <span>Waiting for the ionosonde network…</span>;
  }
  const sign = medianDeltaMhz >= 0 ? "+" : "";
  return (
    <span>
      {stations} sounding sites · {above} of {cells} compared cells sit above the forecast · median
      Δ {sign}
      {medianDeltaMhz.toFixed(1)} MHz · {source ?? "ionosonde network"}
    </span>
  );
}

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
      <p className="px-2 py-2 font-mono text-[10px] text-ink-faint">
        No reflection points yet. A path needs a decode that carries a grid square — reports and 73s
        do not.
      </p>
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
              {cell.measuredMuf3000Mhz === null ? "—" : `≥ ${cell.measuredMuf3000Mhz.toFixed(1)}`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
