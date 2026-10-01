import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { Readout, Readouts } from "../../components/face/Readouts";
import { type Colormap, DEFAULT_COLORMAP } from "../../gl/surface";
import { clearRadarTracks } from "../../lib/api";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { readingOf, useProcessorStore } from "../../lib/processors";
import { clearAction, failAction } from "../../lib/refusals";
import type { ArrayStatus, PatchNode, PatchNodeOf, RadarUpdate } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf, hasWire } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { ProcessorError } from "./ProcessorHealth";
import {
  ageLabel,
  NO_CATALOG,
  processorGate,
  processorLanes,
  useProcessorEdit,
} from "./processorFace";
import { RadarPlot } from "./RadarPlot";
import { RadarChips, RadarSettings } from "./RadarSettings";
import {
  adsbCell,
  bearingCell,
  clutterText,
  confirmedTracks,
  isStale,
  loadText,
  lostCpis,
  lostTitle,
  RADAR_TX_PORT,
  radarChips,
  radarSubtitle,
  referenceText,
  sortTracks,
  TRACK_ROWS,
} from "./radar";

const AGE_TICK_MS = 1_000;
const CLEAR_ACTION = "Clear";
const TRACK_HEADS = ["Track", "km", "m/s", "Bearing", "dB", "ADS-B"] as const;

function useTrackClear(node: string): { clear: () => void; pending: boolean } {
  const mutation = useMutation({
    mutationFn: () => clearRadarTracks(node),
    onSuccess: () => clearAction(node, CLEAR_ACTION),
    onError: (error) => failAction(node, CLEAR_ACTION, error),
  });
  return { clear: () => mutation.mutate(), pending: mutation.isPending };
}

export function RadarFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const edit = useProcessorEdit(node as PatchNodeOf<"passive_radar">);
  const [colormap, setColormap] = useState<Colormap>(DEFAULT_COLORMAP);
  const [selected, setSelected] = useState<number | null>(null);
  if (node.kind !== "passive_radar") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const update = readingOf(state, "passive_radar");
  const stale = state !== undefined && update !== null && isStale(state.receivedAt, update, now);
  const lanes = processorLanes(workspace.graph, array, status);
  const health = processorStatusOf(status, node.id);
  const subtitle = radarSubtitle({
    wired: array !== null,
    txWired: hasWire(workspace.graph, node.id, RADAR_TX_PORT),
    gate: processorGate(status, node.id),
    update,
    stale,
    illuminator: settings?.illuminator.kind ?? "fm",
    lanes,
  });
  return (
    <NodeShell
      node={node}
      title="Passive radar"
      category="tool"
      subtitle={<span className={subtitle.warn ? "text-warn" : undefined}>{subtitle.text}</span>}
    >
      <FaceBody scroll={false}>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">
            <RadarPlot
              node={node.id}
              known={health !== null}
              update={update}
              dim={stale || update === null}
              colormap={colormap}
              selected={selected}
            />
            <RadarChips
              settings={settings}
              edit={edit}
              lanes={Math.max(lanes, 2)}
              colormap={colormap}
              onColormap={setColormap}
            />
            <RadarStrip update={update} status={status} receivedAt={state?.receivedAt} now={now} />
            <TrackTable update={update} selected={selected} onSelect={setSelected} />
            <ProcessorError status={health} />
            <div className="max-h-56 shrink-0 overflow-y-auto">
              <RadarSettings settings={settings} edit={edit} />
            </div>
          </div>
        )}
      </FaceBody>
      <ProcessorFooter
        status={health}
        chips={radarChips(update)}
        actions={<ClearTracks node={node.id} cleared={update === null} />}
      />
    </NodeShell>
  );
}

function ClearTracks({ node, cleared }: { node: string; cleared: boolean }) {
  const { clear, pending } = useTrackClear(node);
  return (
    <Button
      type="button"
      className={BTN}
      title="Forget every track"
      disabled={pending || cleared}
      onClick={clear}
    >
      Clear
    </Button>
  );
}

function RadarStrip({
  update,
  status,
  receivedAt,
  now,
}: {
  update: RadarUpdate | null;
  status: ArrayStatus | undefined;
  receivedAt: number | undefined;
  now: number;
}) {
  const clutter = clutterText(update?.health.suppression_db ?? []);
  const lost =
    update === null ? 0 : lostCpis(update.health, update.axes, status?.sample_rate ?? null);
  const load = update?.health.load ?? 0;
  return (
    <Readouts columns={4}>
      <Readout label="Targets">{String(confirmedTracks(update))}</Readout>
      <Readout label="Clutter" title={clutter.title}>
        {clutter.value}
      </Readout>
      <Readout label="Load" tone={load > 1 ? "danger" : undefined} title="Share of real time spent">
        {update === null ? "-" : loadText(load)}
      </Readout>
      <Readout label="Ref">{update === null ? "-" : referenceText(update)}</Readout>
      <Readout label="CPI">{update === null ? "-" : `${update.axes.cpi_ms.toFixed(0)} ms`}</Readout>
      <Readout label="Age">{ageLabel(receivedAt, now)}</Readout>
      {update !== null && lost > 0 && (
        <Readout label="Drops" tone="danger" title={lostTitle(update.health)}>
          {String(lost)}
        </Readout>
      )}
    </Readouts>
  );
}

function TrackTable({
  update,
  selected,
  onSelect,
}: {
  update: RadarUpdate | null;
  selected: number | null;
  onSelect: (id: number | null) => void;
}) {
  const tracks = sortTracks(update?.tracks ?? [], TRACK_ROWS);
  if (tracks.length === 0) {
    return null;
  }
  return (
    <table className="w-full shrink-0 border-t border-line">
      <thead>
        <tr>
          {TRACK_HEADS.map((head) => (
            <th key={head} className={TABLE_HEAD}>
              {head}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {tracks.map((track) => {
          const bearing = bearingCell(track.aoa);
          const chosen = track.id === selected;
          return (
            <tr
              key={track.id}
              className={`${chosen ? "bg-accent/15" : "hover:bg-panel-2"} ${
                track.state === "coasting" ? "opacity-50" : ""
              }`}
            >
              <td className={TABLE_CELL}>
                <Button
                  type="button"
                  aria-pressed={chosen}
                  title={track.state === "coasting" ? "Coasting" : "Show on the plot"}
                  className="cursor-pointer font-mono hover:text-accent"
                  onClick={() => onSelect(chosen ? null : track.id)}
                >
                  T{track.id}
                </Button>
              </td>
              <td className={TABLE_CELL}>{track.range_km.toFixed(1)}</td>
              <td className={TABLE_CELL}>{track.range_rate_mps.toFixed(0)}</td>
              <td
                className={`${TABLE_CELL} ${bearing.arrayFrame ? "text-ink-faint" : ""}`}
                title={bearing.arrayFrame ? "Array frame" : undefined}
              >
                {bearing.text}
              </td>
              <td className={TABLE_CELL}>{track.snr_db.toFixed(0)}</td>
              <td className={`${TABLE_CELL} max-w-20 truncate`}>{adsbCell(track)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
