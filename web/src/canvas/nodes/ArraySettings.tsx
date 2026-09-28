import { useMutation } from "@tanstack/react-query";
import { Checkbox } from "../../components/Checkbox";
import { CONTROL_W, type Options } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { tuneArray } from "../../lib/api";
import { clearAction, failAction } from "../../lib/refusals";
import type {
  ArrayCal,
  ArrayCalSource,
  ArrayGain,
  ArrayNode,
  ArrayOrientation,
  ArrayStatus,
  DeviceSet,
} from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { ArrayGeometryEditor } from "./ArrayGeometryEditor";
import {
  calSourceOf,
  checkOptions,
  headingLabel,
  switchedOrientation,
  TIER_TEXT,
  tierOptions,
} from "./arrayNode";
import { FoldSection } from "./FoldSection";

export const GAIN_ACTION = "Gain";
export const MIN_ARRAY_GAIN_DB = -20;
export const MAX_ARRAY_GAIN_DB = 80;
export const NO_HEADING = "No heading";

const MAX_CAL_OFFSET_HZ = 50_000_000;
const MIN_CAL_WIDTH_HZ = 100;
const MAX_CAL_WIDTH_HZ = 2_000_000;

type Edit = (next: Partial<ArrayNode>) => void;

export function useArrayEdit(id: string): Edit {
  const workspace = useWorkspaceContext();
  return (next) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, id, (stored) =>
        stored.kind === "array" ? { ...stored, data: { ...stored.data, ...next } } : stored,
      ),
    }));
    workspace.apply();
  };
}

export function useArrayGain(node: string): {
  setGain: (gain: ArrayGain) => void;
  pending: boolean;
} {
  const gain = useMutation({
    mutationFn: (next: ArrayGain) => tuneArray(node, { gain: next }),
    onSuccess: () => clearAction(node, GAIN_ACTION),
    onError: (error) => failAction(node, GAIN_ACTION, error),
  });
  return { setGain: (next) => gain.mutate(next), pending: gain.isPending };
}

export interface ArraySettingsProps {
  node: string;
  data: ArrayNode;
  status: ArrayStatus | undefined;
  lanes: number;
  members: readonly DeviceSet[];
  memberCount: number;
  positionWired: boolean;
  edit: Edit;
}

export function ArraySettings(props: ArraySettingsProps) {
  const { data, status, lanes, edit } = props;
  return (
    <div className="flex flex-col">
      <FoldSection label="Geometry">
        <ArrayGeometryEditor
          geometry={data.geometry}
          lanes={lanes}
          azimuthDeg={status?.azimuth_deg ?? null}
          centerHz={status?.center_hz ?? null}
          onChange={(geometry) => edit({ geometry })}
        />
      </FoldSection>
      <FoldSection label="Orientation">
        <OrientationSettings
          orientation={data.orientation}
          status={status}
          positionWired={props.positionWired}
          edit={edit}
        />
      </FoldSection>
      <FoldSection label="Calibration">
        <CalibrationSettings cal={data.cal} edit={edit} />
      </FoldSection>
      <FoldSection label="Gain">
        <GainSettings node={props.node} status={status} />
      </FoldSection>
      <FoldSection label="Tier">
        <TierSettings
          declared={data.declared}
          members={props.members}
          memberCount={props.memberCount}
          status={status}
          edit={edit}
        />
      </FoldSection>
    </div>
  );
}

const ORIENTATION_OPTIONS: Options<ArrayOrientation["kind"]> = [
  { value: "fixed", label: "Fixed", title: "Array points one way" },
  { value: "heading", label: "Heading", title: "Array follows a GPS heading" },
];

function OrientationSettings({
  orientation,
  status,
  positionWired,
  edit,
}: {
  orientation: ArrayOrientation;
  status: ArrayStatus | undefined;
  positionWired: boolean;
  edit: Edit;
}) {
  const pick = (kind: ArrayOrientation["kind"]): void => {
    const next = switchedOrientation(kind, orientation, status?.azimuth_deg ?? null);
    if (next !== orientation) {
      edit({ orientation: next });
    }
  };
  return (
    <Settings>
      <SettingRow label="Mode">
        <Segmented
          label="Orientation"
          value={orientation.kind}
          options={ORIENTATION_OPTIONS}
          onChange={pick}
        />
      </SettingRow>
      {orientation.kind === "fixed" ? (
        <SettingRow label="Azimuth" title="Array forward, from true north">
          <NumberField
            label="Azimuth"
            unit="°"
            value={orientation.azimuth_deg}
            min={0}
            max={359.9}
            step={0.1}
            onCommit={(azimuth_deg) => edit({ orientation: { kind: "fixed", azimuth_deg } })}
          />
        </SettingRow>
      ) : (
        <>
          <SettingRow label="Mount" title="Array forward relative to the heading source">
            <NumberField
              label="Mount"
              unit="°"
              value={orientation.mount_offset_deg}
              min={-180}
              max={180}
              step={0.1}
              onCommit={(mount_offset_deg) =>
                edit({ orientation: { kind: "heading", mount_offset_deg } })
              }
            />
          </SettingRow>
          <SettingRow label="Heading">
            {positionWired ? (
              <span className="font-mono text-xs">{headingLabel(orientation, status)}</span>
            ) : (
              <span className="text-xs text-danger" title="Wire a GPS with heading, like a phone">
                {NO_HEADING}
              </span>
            )}
          </SettingRow>
        </>
      )}
    </Settings>
  );
}

const CAL_SOURCES: Options<ArrayCalSource["kind"]> = [
  { value: "noise", label: "Noise", title: "Built-in noise source" },
  { value: "pilot", label: "Pilot", title: "A carrier at a known offset" },
  { value: "emitter", label: "Emitter", title: "A transmitter at a known bearing" },
  { value: "off", label: "Off", title: "No calibration" },
];

function CalibrationSettings({ cal, edit }: { cal: ArrayCal; edit: Edit }) {
  const source = cal.source;
  const setCal = (next: Partial<ArrayCal>): void => edit({ cal: { ...cal, ...next } });
  const setSource = (next: ArrayCalSource): void => setCal({ source: next });
  return (
    <Settings>
      <SettingRow label="Source">
        <Select
          label="Calibration source"
          value={source.kind}
          options={CAL_SOURCES}
          onChange={(kind) => setSource(calSourceOf(kind, source))}
        />
      </SettingRow>
      {source.kind === "emitter" && (
        <SettingRow label="Bearing" title="True bearing to the transmitter">
          <NumberField
            label="Bearing"
            unit="°"
            value={source.bearing_deg}
            min={0}
            max={359.9}
            step={0.1}
            onCommit={(bearing_deg) => setSource({ ...source, bearing_deg })}
          />
        </SettingRow>
      )}
      {(source.kind === "pilot" || source.kind === "emitter") && (
        <>
          <SettingRow label="Offset" title="From the array centre">
            <NumberField
              label="Offset"
              unit="Hz"
              value={source.offset_hz}
              min={-MAX_CAL_OFFSET_HZ}
              max={MAX_CAL_OFFSET_HZ}
              step={1}
              onCommit={(offset_hz) => setSource({ ...source, offset_hz })}
            />
          </SettingRow>
          <SettingRow label="Width">
            <NumberField
              label="Width"
              unit="Hz"
              value={source.bandwidth_hz}
              min={MIN_CAL_WIDTH_HZ}
              max={MAX_CAL_WIDTH_HZ}
              step={1}
              onCommit={(bandwidth_hz) => setSource({ ...source, bandwidth_hz })}
            />
          </SettingRow>
        </>
      )}
      <SettingRow label="Check" title="Recheck phase this often">
        <Select
          label="Check"
          value={cal.check_s}
          options={checkOptions(cal.check_s)}
          onChange={(check_s) => setCal({ check_s })}
        />
      </SettingRow>
      <SettingRow label="EQ" title="Flatten each lane across the band">
        <Checkbox
          label="Per-bin equaliser"
          checked={cal.equaliser}
          onChange={(equaliser) => setCal({ equaliser })}
        />
      </SettingRow>
      <SettingRow label="Warm start" title="Start from the last stored calibration">
        <Checkbox
          label="Warm start"
          checked={cal.warm_start}
          onChange={(warm_start) => setCal({ warm_start })}
        />
      </SettingRow>
    </Settings>
  );
}

function GainSettings({ node, status }: { node: string; status: ArrayStatus | undefined }) {
  const { setGain, pending } = useArrayGain(node);
  const range = status?.gain_range_db;
  const auto = status?.gain.kind === "auto";
  const held =
    status?.gain.kind === "manual" ? status.gain.db : (status?.gain_db ?? MIN_ARRAY_GAIN_DB);
  const off = status === undefined || pending;
  return (
    <Settings>
      <SettingRow label="Gain" title="One gain for every lane">
        <NumberField
          label="Gain"
          unit="dB"
          value={held}
          min={Math.max(MIN_ARRAY_GAIN_DB, range?.min ?? MIN_ARRAY_GAIN_DB)}
          max={Math.min(MAX_ARRAY_GAIN_DB, range?.max ?? MAX_ARRAY_GAIN_DB)}
          step={range?.step ?? 0.1}
          disabled={off || auto}
          onCommit={(db) => setGain({ kind: "manual", db })}
        />
      </SettingRow>
      <SettingRow label="Auto" title="Radio AGC on every lane">
        <Checkbox
          label="Auto gain"
          checked={auto}
          disabled={off}
          onChange={(on) => setGain(on ? { kind: "auto" } : { kind: "manual", db: held })}
        />
      </SettingRow>
    </Settings>
  );
}

function TierSettings({
  declared,
  members,
  memberCount,
  status,
  edit,
}: {
  declared: ArrayNode["declared"];
  members: readonly DeviceSet[];
  memberCount: number;
  status: ArrayStatus | undefined;
  edit: Edit;
}) {
  if (memberCount < 2) {
    const tier = status?.tier ?? members[0]?.capabilities.coherence ?? "none";
    return (
      <Settings>
        <SettingRow label="Radio" title="One radio sets the tier">
          <span className="font-mono text-xs">{TIER_TEXT[tier]}</span>
        </SettingRow>
      </Settings>
    );
  }
  return (
    <Settings>
      <SettingRow label="Tier" title="What the radios share">
        <Select
          label="Array tier"
          value={declared}
          options={tierOptions(declared)}
          className={declared === "none" ? `${CONTROL_W} text-danger` : CONTROL_W}
          onChange={(next) => edit({ declared: next })}
        />
      </SettingRow>
    </Settings>
  );
}
