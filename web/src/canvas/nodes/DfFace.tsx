import { Button } from "../../components/BaseControls";
import { BTN, type Options } from "../../components/controls";
import { formatSignedKhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { TextField } from "../../components/TextField";
import { calibrateCoherent } from "../../lib/api";
import { useDfStore } from "../../lib/df";
import type { DfAlgorithm, DfParams, PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import {
  beamAzimuth,
  beamMode,
  bearingLabel,
  CAL_VERDICT_TEXT,
  COMPASS_MARKS,
  calVerdict,
  DEFAULT_DF_PARAMS,
  elementCount,
  geometryOf,
  laneQualityPercent,
  polarPoint,
  spectrumPath,
  tierLabel,
  withCount,
} from "./df";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

const ALGORITHMS: Options<DfAlgorithm> = [
  { value: "correlative", label: "Beamformer" },
  { value: "music", label: "MUSIC" },
];

const GEOMETRIES: Options<"uca" | "ula"> = [
  { value: "uca", label: "Circle" },
  { value: "ula", label: "Line" },
];

const BEAMS: Options<"follow" | "fixed"> = [
  { value: "follow", label: "Follow bearing" },
  { value: "fixed", label: "Fixed azimuth" },
];

const SIZE = 220;
const CENTRE = SIZE / 2;
const INNER = 26;
const OUTER = SIZE / 2 - 20;

export function DfFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useDfStore((store) => store.byNode[node.id]);
  if (node.kind !== "df") {
    return null;
  }
  const settings = node.data.settings ?? DEFAULT_DF_PARAMS;
  const update = (next: Partial<DfParams>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "df"
          ? {
              ...current,
              data: { settings: { ...(current.data.settings ?? DEFAULT_DF_PARAMS), ...next } },
            }
          : current,
      ),
    }));
  };
  const verdict = calVerdict(state?.cal);
  const bearing = verdict === "phase_unknown" ? null : (state?.reading ?? null);
  return (
    <NodeShell
      node={node}
      title="Direction finder"
      category="channel"
      subtitle={`${elementCount(settings.geometry)} elements · ${tierLabel(state?.cal)}`}
      live={bearing !== null && bearing.confidence > 0}
    >
      <FaceBody>
        {state === undefined ? (
          <FaceEmpty>
            Wire every element of the array to one coherent radio, then apply the patch.
          </FaceEmpty>
        ) : (
          <div className="flex flex-col items-center gap-2 p-2">
            <CompassRose
              spectrum={bearing?.pseudospectrum ?? []}
              bearingDeg={bearing?.bearing_deg ?? null}
            />
            <Readout>
              <ReadoutRow label="Bearing">
                {bearing === null ? "—" : bearingLabel(bearing.bearing_deg)}
              </ReadoutRow>
              <ReadoutRow label="Confidence">
                {bearing === null ? "—" : `${Math.round(bearing.confidence * 100)}%`}
              </ReadoutRow>
              <ReadoutRow label="Calibration">{CAL_VERDICT_TEXT[verdict]}</ReadoutRow>
            </Readout>
            <LaneStrip cal={state.cal} />
            <Button
              className={BTN}
              type="button"
              onClick={() => {
                void calibrateCoherent(node.id);
              }}
            >
              Calibrate
            </Button>
          </div>
        )}
        <DfSettings
          settings={settings}
          bearingDeg={bearing?.bearing_deg ?? null}
          onChange={update}
        />
      </FaceBody>
    </NodeShell>
  );
}

function CompassRose({
  spectrum,
  bearingDeg,
}: {
  spectrum: readonly number[];
  bearingDeg: number | null;
}) {
  const needle = bearingDeg === null ? null : polarPoint(bearingDeg, OUTER, CENTRE);
  return (
    <svg
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      className="h-52 w-52"
      role="img"
      aria-label="Direction finding compass"
    >
      <title>Direction finding compass</title>
      <circle cx={CENTRE} cy={CENTRE} r={OUTER} className="fill-none stroke-line" />
      <circle
        cx={CENTRE}
        cy={CENTRE}
        r={(OUTER + INNER) / 2}
        className="fill-none stroke-line/50"
      />
      {COMPASS_MARKS.map((mark) => {
        const label = polarPoint(mark.bearing, OUTER + 10, CENTRE);
        const tick = polarPoint(mark.bearing, OUTER, CENTRE);
        const root = polarPoint(mark.bearing, OUTER - 6, CENTRE);
        return (
          <g key={mark.label}>
            <line x1={root.x} y1={root.y} x2={tick.x} y2={tick.y} className="stroke-line" />
            <text
              x={label.x}
              y={label.y}
              className="fill-ink-dim text-[8px]"
              textAnchor="middle"
              dominantBaseline="middle"
            >
              {mark.label}
            </text>
          </g>
        );
      })}
      {spectrum.length > 0 && (
        <path
          d={spectrumPath(spectrum, CENTRE, INNER, OUTER)}
          className="fill-accent/20 stroke-accent"
        />
      )}
      {needle !== null && (
        <line
          x1={CENTRE}
          y1={CENTRE}
          x2={needle.x}
          y2={needle.y}
          className="stroke-accent stroke-2"
        />
      )}
      <circle cx={CENTRE} cy={CENTRE} r={2} className="fill-accent" />
    </svg>
  );
}

function LaneStrip({ cal }: { cal: { lanes: readonly { quality: number }[] } }) {
  if (cal.lanes.length === 0) {
    return null;
  }
  return (
    <div className="flex w-full gap-1" aria-label="Per-lane calibration quality">
      {cal.lanes.map((lane, index) => (
        <div
          // biome-ignore lint/suspicious/noArrayIndexKey: a lane is identified by its position
          key={index}
          className="h-1.5 flex-1 rounded-full bg-line"
          title={`Lane ${index + 1}: ${laneQualityPercent(lane.quality)}%`}
        >
          <div
            className="h-full rounded-full bg-accent"
            style={{ width: `${laneQualityPercent(lane.quality)}%` }}
          />
        </div>
      ))}
    </div>
  );
}

function DfSettings({
  settings,
  bearingDeg,
  onChange,
}: {
  settings: DfParams;
  bearingDeg: number | null;
  onChange: (next: Partial<DfParams>) => void;
}) {
  return (
    <Settings className="border-t border-line p-2">
      <SettingRow label="Algorithm">
        <Select
          label="Direction finding algorithm"
          value={settings.algorithm}
          onChange={(algorithm) => onChange({ algorithm })}
          options={ALGORITHMS}
        />
      </SettingRow>
      <SettingRow label="Geometry">
        <Select
          label="Array geometry"
          value={settings.geometry.kind}
          onChange={(kind) => onChange({ geometry: geometryOf(kind, settings.geometry) })}
          options={GEOMETRIES}
        />
      </SettingRow>
      {settings.geometry.kind === "uca" && (
        <SettingRow label="Radius">
          <NumberField
            label="Array radius in metres"
            value={settings.geometry.radius_m}
            min={0.01}
            max={100}
            step={0.01}
            onCommit={(radius_m) =>
              onChange({
                geometry: { kind: "uca", radius_m, count: elementCount(settings.geometry) },
              })
            }
          />
        </SettingRow>
      )}
      {settings.geometry.kind === "ula" && (
        <SettingRow label="Spacing">
          <NumberField
            label="Element spacing in metres"
            value={settings.geometry.spacing_m}
            min={0.01}
            max={100}
            step={0.01}
            onCommit={(spacing_m) =>
              onChange({
                geometry: { kind: "ula", spacing_m, count: elementCount(settings.geometry) },
              })
            }
          />
        </SettingRow>
      )}
      <SettingRow label="Elements">
        <NumberField
          label="Element count"
          value={elementCount(settings.geometry)}
          min={2}
          max={16}
          step={1}
          onCommit={(count) => onChange({ geometry: withCount(settings.geometry, count) })}
        />
      </SettingRow>
      <SettingRow label="Offset" title={formatSignedKhz(settings.offset_hz)}>
        <NumberField
          label="Signal offset in hertz"
          value={settings.offset_hz}
          step={1_000}
          onCommit={(offset_hz) => onChange({ offset_hz })}
        />
      </SettingRow>
      <SettingRow label="Bandwidth">
        <NumberField
          label="Signal bandwidth in hertz"
          value={settings.bandwidth_hz}
          min={100}
          max={20_000_000}
          step={1_000}
          onCommit={(bandwidth_hz) => onChange({ bandwidth_hz })}
        />
      </SettingRow>
      <SettingRow label="Report every">
        <NumberField
          label="Report interval in milliseconds"
          value={settings.report_ms}
          min={100}
          max={10_000}
          step={100}
          onCommit={(report_ms) => onChange({ report_ms })}
        />
      </SettingRow>
      <SettingRow label="Station">
        <TextField
          label="What this receiver calls itself when a bearing leaves it"
          value={settings.station_id ?? ""}
          placeholder="unnamed"
          onCommit={(name) => onChange({ station_id: name === "" ? null : name })}
        />
      </SettingRow>
      <SettingRow label="Beam">
        <Select
          label="Where the beam output points"
          value={beamMode(settings.beam_bearing_deg)}
          onChange={(mode) => onChange({ beam_bearing_deg: beamAzimuth(mode, bearingDeg) })}
          options={BEAMS}
        />
      </SettingRow>
      {settings.beam_bearing_deg != null && (
        <SettingRow label="Azimuth">
          <NumberField
            label="Beam azimuth in degrees"
            value={settings.beam_bearing_deg}
            min={0}
            max={359}
            step={1}
            onCommit={(beam_bearing_deg) => onChange({ beam_bearing_deg })}
          />
        </SettingRow>
      )}
    </Settings>
  );
}
