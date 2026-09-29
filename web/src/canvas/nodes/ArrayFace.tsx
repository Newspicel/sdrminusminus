import { useMutation } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_PRIMARY, type Options } from "../../components/controls";
import { ANY_FREQUENCY, tuningRange } from "../../components/dial";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { formatMhz } from "../../components/format";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Segmented } from "../../components/Segmented";
import { calibrateArray, startArrayRecording, stopArrayRecording } from "../../lib/api";
import { failureText, shownCenterHz, useArrayStore } from "../../lib/arrays";
import { clearAction, failAction } from "../../lib/refusals";
import type {
  ArrayNode,
  ArrayStatus,
  ArrayTuningMode,
  DeviceSet,
  PatchNode,
} from "../../lib/types";
import { useArrayTune } from "../../lib/useArrayTune";
import { useNow } from "../../lib/useNow";
import { hasWire } from "../binding";
import { useWorkspaceContext } from "../context";
import { arrayWiredLanes } from "../graph";
import { ArrayLanes } from "./ArrayLanes";
import { ArraySettings, useArrayEdit } from "./ArraySettings";
import {
  arrayLaneRows,
  arraySpanHz,
  arraySubtitle,
  calLabel,
  calTitle,
  failureTitle,
  headingLabel,
  laneGaps,
  memberDevices,
  recordingLabel,
  syncLabel,
  tierLabel,
} from "./arrayNode";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

const AGE_TICK_MS = 1_000;

export const CALIBRATE_ACTION = "Calibrate";
export const RECORD_ACTION = "Rec";

const TUNING_OPTIONS: Options<ArrayTuningMode> = [
  { value: "together", label: "Together", title: "Every lane on one frequency" },
  { value: "spread", label: "Spread", title: "Lanes side by side" },
];

function useArrayAction(node: string, action: string, run: () => Promise<unknown>) {
  return useMutation({
    mutationFn: run,
    onSuccess: () => clearAction(node, action),
    onError: (error) => failAction(node, action, error),
  });
}

export function ArrayFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const status = useArrayStore((store) => store.byNode[node.id]);
  const now = useNow(AGE_TICK_MS);
  const edit = useArrayEdit(node.id);
  const calibrate = useArrayAction(node.id, CALIBRATE_ACTION, () => calibrateArray(node.id));
  const record = useArrayAction(node.id, RECORD_ACTION, () => startArrayRecording(node.id));
  const stop = useArrayAction(node.id, RECORD_ACTION, () => stopArrayRecording(node.id));
  if (node.kind !== "array") {
    return null;
  }
  const graph = workspace.graph;
  const lanes = arrayWiredLanes(graph, node.id);
  const rows = arrayLaneRows(graph, workspace.devices, node.id, status);
  const memberNodes = memberDevices(graph, node.id);
  const members = memberNodes.flatMap((member) => {
    const set = workspace.devices.get(member);
    return set === undefined ? [] : [set];
  });
  const recording = status?.recording ?? null;
  const uncalibrated = status === undefined || status.cal === "none" || status.cal === "failed";
  return (
    <NodeShell
      node={node}
      title="Array"
      category="tool"
      subtitle={arraySubtitle(status, rows.length)}
    >
      <FaceBody>
        <ArrayDial
          node={node.id}
          data={node.data}
          status={status}
          lead={members[0]}
          span={arraySpanHz(graph, workspace.devices, node.id, status)}
          edit={edit}
        />
        {status !== undefined && (
          <ArrayStatusReadout status={status} orientation={node.data.orientation} now={now} />
        )}
        <ArrayLanes rows={rows} />
        <ArraySettings
          node={node.id}
          data={node.data}
          status={status}
          lanes={lanes}
          members={members}
          memberCount={memberNodes.length}
          positionWired={hasWire(graph, node.id, "position")}
          edit={edit}
        />
      </FaceBody>
      <FaceFooter>
        {recording === null ? (
          <Button
            type="button"
            className={BTN}
            title="Record every lane to one collection"
            disabled={status === undefined || record.isPending}
            onClick={() => record.mutate()}
          >
            Rec
          </Button>
        ) : (
          <Button
            type="button"
            className={BTN}
            title={recording.stem}
            disabled={stop.isPending}
            onClick={() => stop.mutate()}
          >
            Stop
          </Button>
        )}
        <Button
          type="button"
          className={uncalibrated ? BTN_PRIMARY : BTN}
          title="Line up lane phase and gain"
          disabled={calibrate.isPending || rows.length === 0}
          onClick={() => calibrate.mutate()}
        >
          Calibrate
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}

function ArrayDial({
  node,
  data,
  status,
  lead,
  span,
  edit,
}: {
  node: string;
  data: ArrayNode;
  status: ArrayStatus | undefined;
  lead: DeviceSet | undefined;
  span: number | null;
  edit: (next: Partial<ArrayNode>) => void;
}) {
  const { tuneArray } = useArrayTune();
  const centerHz = useArrayStore((store) => shownCenterHz(store, node));
  const active = useFaceActive();
  return (
    <div className="@container flex min-w-0 flex-col gap-2 p-2">
      <FrequencyDial
        id={dialId(node, 0)}
        hz={centerHz ?? 0}
        range={lead === undefined ? ANY_FREQUENCY : tuningRange(lead.capabilities)}
        disabled={status === undefined}
        wheelTunes={active}
        onTune={(hz) => tuneArray(node, hz)}
      />
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          label="Lane tuning"
          value={data.tuning}
          options={TUNING_OPTIONS}
          onChange={(tuning) => edit({ tuning })}
        />
        {data.tuning === "spread" && (
          <span className="font-mono text-xs text-ink-dim" title="Band the lanes cover together">
            Span {span === null ? "-" : formatMhz(span)}
          </span>
        )}
      </div>
    </div>
  );
}

function ArrayStatusReadout({
  status,
  orientation,
  now,
}: {
  status: ArrayStatus;
  orientation: ArrayNode["orientation"];
  now: number;
}) {
  const gaps = laneGaps(status);
  const failure = status.failure ?? null;
  const recording = status.recording ?? null;
  return (
    <Readout>
      <ReadoutRow label="Sync" title="Lanes lined up in time">
        {syncLabel(status)}
      </ReadoutRow>
      <ReadoutRow label="Tier" title="Capped: measured drift allows less than declared">
        {tierLabel(status)}
      </ReadoutRow>
      <ReadoutRow label="Cal">
        <span
          className={status.cal === "failed" ? "text-danger" : undefined}
          title={calTitle(status)}
        >
          {calLabel(status, now)}
        </span>
      </ReadoutRow>
      <ReadoutRow label="Heading" title="Array forward, true north">
        {headingLabel(orientation, status)}
      </ReadoutRow>
      {gaps > 0 && (
        <ReadoutRow label="Gaps" title="Samples lost and resynced">
          {gaps}
        </ReadoutRow>
      )}
      {status.realigns > 0 && <ReadoutRow label="Realigns">{status.realigns}</ReadoutRow>}
      {status.dropped_samples > 0 && (
        <ReadoutRow label="Drops" title="Samples dropped">
          {status.dropped_samples}
        </ReadoutRow>
      )}
      {status.events_lost > 0 && (
        <ReadoutRow label="Lost" title="Lane events lost">
          {status.events_lost}
        </ReadoutRow>
      )}
      {recording !== null && (
        <ReadoutRow label="Rec">
          <span
            className={recording.dropped > 0 || recording.error != null ? "text-danger" : undefined}
            title={recording.error ?? recording.stem}
          >
            {recordingLabel(recording, status.sample_rate)}
          </span>
        </ReadoutRow>
      )}
      {failure !== null && (
        <ReadoutRow label="Fault">
          <span role="alert" className="block truncate text-danger" title={failureTitle(failure)}>
            {failureText(failure)}
          </span>
        </ReadoutRow>
      )}
    </Readout>
  );
}
