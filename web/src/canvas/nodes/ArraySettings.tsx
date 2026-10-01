import { useMutation } from "@tanstack/react-query";
import { Checkbox } from "../../components/Checkbox";
import type { Options } from "../../components/controls";
import { Chips, ChoiceChip, ReadoutChip, SettingChip } from "../../components/face/Chips";
import { FoldSection, SettingsFold } from "../../components/face/Fold";
import { formatMhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { tuneArray } from "../../lib/api";
import { ARRAY_LIMITS } from "../../lib/limits";
import { clearAction, failAction } from "../../lib/refusals";
import type {
  ArrayCal,
  ArrayCalSource,
  ArrayGain,
  ArrayNode,
  ArrayOrientation,
  ArrayStatus,
  ArrayTuningMode,
  DeviceSet,
} from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { ArrayGeometryEditor } from "./ArrayGeometryEditor";
import {
  calSourceOf,
  checkOptions,
  degreesText,
  headingLabel,
  switchedOrientation,
  TIER_TEXT,
  tierOptions,
} from "./arrayNode";

export const GAIN_ACTION = "Gain";
export const NO_HEADING = "No heading";
export const NO_HEADING_TITLE = "Wire a GPS with heading, like a phone";
export const TIER_TITLE = "What the radios share";

const CAL_OFFSET_HZ = ARRAY_LIMITS.cal_offset_hz;
const CAL_WIDTH_HZ = ARRAY_LIMITS.cal_bandwidth_hz;

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

const TUNING_OPTIONS: Options<ArrayTuningMode> = [
  { value: "together", label: "Together", title: "Every lane on one frequency" },
  { value: "spread", label: "Spread", title: "Lanes side by side" },
];

const CAL_SOURCES: Options<ArrayCalSource["kind"]> = [
  { value: "noise", label: "Noise", title: "Built-in noise source" },
  { value: "pilot", label: "Pilot", title: "A carrier at a known offset" },
  { value: "emitter", label: "Emitter", title: "A transmitter at a known bearing" },
  { value: "off", label: "Off", title: "No calibration" },
];

export interface ArrayChipsProps {
  data: ArrayNode;
  status: ArrayStatus | undefined;
  members: readonly DeviceSet[];
  memberCount: number;
  positionWired: boolean;
  span: number | null;
  edit: Edit;
}

export function ArrayChips(props: ArrayChipsProps) {
  const { data, status, edit } = props;
  const cal = data.cal;
  return (
    <Chips className="px-2 pb-2">
      <ChoiceChip
        label="Tuning"
        value={data.tuning}
        options={TUNING_OPTIONS}
        title="Lane tuning"
        onChange={(tuning) => edit({ tuning })}
      />
      {data.tuning === "spread" && (
        <ReadoutChip
          label="Span"
          value={props.span === null ? "-" : formatMhz(props.span)}
          title="Band the lanes cover together"
        />
      )}
      <OrientationChip
        orientation={data.orientation}
        status={status}
        positionWired={props.positionWired}
        edit={edit}
      />
      <ChoiceChip
        label="Cal source"
        value={cal.source.kind}
        options={CAL_SOURCES}
        title="Calibration source"
        quiet={cal.source.kind === "off"}
        onChange={(kind) => edit({ cal: { ...cal, source: calSourceOf(kind, cal.source) } })}
      />
      <TierChip
        declared={data.declared}
        members={props.members}
        memberCount={props.memberCount}
        status={status}
        edit={edit}
      />
    </Chips>
  );
}

const ORIENTATION_OPTIONS: Options<ArrayOrientation["kind"]> = [
  { value: "fixed", label: "Fixed", title: "Array points one way" },
  { value: "heading", label: "Heading", title: "Array follows a GPS heading" },
];

function OrientationChip({
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
  const lost = orientation.kind === "heading" && !positionWired;
  const value =
    orientation.kind === "fixed" ? degreesText(orientation.azimuth_deg) : lost ? NO_HEADING : "GPS";
  return (
    <SettingChip
      label="Forward"
      value={value}
      tone={lost ? "danger" : undefined}
      title={lost ? NO_HEADING_TITLE : "Where the array points"}
      width="w-72"
    >
      {() => (
        <OrientationSettings
          orientation={orientation}
          status={status}
          positionWired={positionWired}
          edit={edit}
        />
      )}
    </SettingChip>
  );
}

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
              <span className="text-xs text-danger" title={NO_HEADING_TITLE}>
                {NO_HEADING}
              </span>
            )}
          </SettingRow>
        </>
      )}
    </Settings>
  );
}

function TierChip({
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
    return status === undefined ? (
      <ReadoutChip label="Tier" value={TIER_TEXT[tier]} title="One radio sets the tier" />
    ) : null;
  }
  return (
    <ChoiceChip
      label="Tier"
      value={declared}
      options={tierOptions(declared)}
      title={TIER_TITLE}
      tone={declared === "none" ? "danger" : undefined}
      onChange={(next) => edit({ declared: next })}
    />
  );
}

export function ArraySettings({
  data,
  status,
  lanes,
  edit,
}: {
  data: ArrayNode;
  status: ArrayStatus | undefined;
  lanes: number;
  edit: Edit;
}) {
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
      <SettingsFold label="Calibration">
        <CalibrationRows cal={data.cal} edit={edit} />
      </SettingsFold>
    </div>
  );
}

function CalibrationRows({ cal, edit }: { cal: ArrayCal; edit: Edit }) {
  const source = cal.source;
  const setCal = (next: Partial<ArrayCal>): void => edit({ cal: { ...cal, ...next } });
  const setSource = (next: ArrayCalSource): void => setCal({ source: next });
  return (
    <>
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
              min={CAL_OFFSET_HZ.min}
              max={CAL_OFFSET_HZ.max}
              step={1}
              onCommit={(offset_hz) => setSource({ ...source, offset_hz })}
            />
          </SettingRow>
          <SettingRow label="Width">
            <NumberField
              label="Width"
              unit="Hz"
              value={source.bandwidth_hz}
              min={CAL_WIDTH_HZ.min}
              max={CAL_WIDTH_HZ.max}
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
    </>
  );
}
