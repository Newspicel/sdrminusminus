import { Collapsible } from "@base-ui/react/collapsible";
import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_QUIET, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { COLOUR_OPTIONS } from "../../components/plotFrame";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { type Colormap, DEFAULT_COLORMAP } from "../../gl/surface";
import { clearRadarTracks } from "../../lib/api";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { readingOf, useProcessorStore } from "../../lib/processors";
import { clearAction, failAction } from "../../lib/refusals";
import type {
  ArrayStatus,
  PassiveRadarParams,
  PatchNode,
  PatchNodeOf,
  RadarUpdate,
} from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf, hasWire } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFaults, ProcessorReadout, ReadoutCell } from "./ProcessorReadout";
import {
  ageLabel,
  NO_CATALOG,
  processorGate,
  processorLanes,
  useProcessorEdit,
} from "./processorFace";
import { RadarPlot } from "./RadarPlot";
import { RadarSettings } from "./RadarSettings";
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
              update={update}
              dim={stale || update === null}
              colormap={colormap}
              selected={selected}
            />
            <RadarStrip update={update} status={status} receivedAt={state?.receivedAt} now={now} />
            <ProcessorFaults status={processorStatusOf(status, node.id)} />
            <TrackTable update={update} selected={selected} onSelect={setSelected} />
            <RadarFooter
              node={node.id}
              settings={settings}
              edit={edit}
              lanes={lanes}
              colormap={colormap}
              onColormap={setColormap}
              cleared={update === null}
            />
          </div>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function RadarFooter({
  node,
  settings,
  edit,
  lanes,
  colormap,
  onColormap,
  cleared,
}: {
  node: string;
  settings: PassiveRadarParams;
  edit: (next: Partial<PassiveRadarParams>) => void;
  lanes: number;
  colormap: Colormap;
  onColormap: (colormap: Colormap) => void;
  cleared: boolean;
}) {
  const { clear, pending } = useTrackClear(node);
  return (
    <Collapsible.Root className="flex shrink-0 flex-col border-t border-line">
      <div className="flex items-center gap-2 px-2 py-1.5">
        <Collapsible.Trigger className={BTN_QUIET}>Settings</Collapsible.Trigger>
        <Button
          type="button"
          className={`${BTN} ml-auto`}
          title="Forget every track"
          disabled={pending || cleared}
          onClick={clear}
        >
          Clear
        </Button>
      </div>
      <Collapsible.Panel className="max-h-56 overflow-y-auto">
        <div className="px-2 pb-2">
          <Settings>
            <SettingRow label="Colours">
              <Select
                label="Colours"
                value={colormap}
                options={COLOUR_OPTIONS}
                onChange={onColormap}
                className="w-28"
              />
            </SettingRow>
          </Settings>
        </div>
        <RadarSettings settings={settings} edit={edit} lanes={Math.max(lanes, 2)} />
      </Collapsible.Panel>
    </Collapsible.Root>
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
    <div className="shrink-0 border-t border-line px-2 py-1.5">
      <ProcessorReadout columns={4}>
        <ReadoutCell label="Targets" value={String(confirmedTracks(update))} />
        <ReadoutCell label="Clutter" value={clutter.value} title={clutter.title} />
        <ReadoutCell
          label="Load"
          value={update === null ? "-" : loadText(load)}
          danger={load > 1}
          title="Share of real time spent"
        />
        <ReadoutCell label="Ref" value={update === null ? "-" : referenceText(update)} />
        <ReadoutCell
          label="CPI"
          value={update === null ? "-" : `${update.axes.cpi_ms.toFixed(0)} ms`}
        />
        <ReadoutCell label="Age" value={ageLabel(receivedAt, now)} />
        {update !== null && lost > 0 && (
          <ReadoutCell label="Drops" value={String(lost)} danger title={lostTitle(update.health)} />
        )}
      </ProcessorReadout>
    </div>
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
