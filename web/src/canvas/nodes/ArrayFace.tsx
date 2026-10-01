import { useMutation } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_PRIMARY } from "../../components/controls";
import { ANY_FREQUENCY, inTuningRange, tuningRange } from "../../components/dial";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { FaceStats, Stat } from "../../components/face/Stats";
import { formatCount, formatMhz } from "../../components/format";
import { TuneTo } from "../../components/TuneTo";
import { calibrateArray, startArrayRecording, stopArrayRecording } from "../../lib/api";
import { failureText, shownCenterHz, useArrayStore } from "../../lib/arrays";
import { clearAction, failAction } from "../../lib/refusals";
import type { ArrayNode, ArrayStatus, DeviceSet, PatchNode } from "../../lib/types";
import { useArrayTune } from "../../lib/useArrayTune";
import { useNow } from "../../lib/useNow";
import { hasWire } from "../binding";
import { useWorkspaceContext } from "../context";
import { arrayWiredLanes } from "../graph";
import { ALIASING, ALIASING_TITLE, aliasing } from "./ArrayGeometryEditor";
import { ArrayLanes } from "./ArrayLanes";
import { ArrayChips, ArraySettings, useArrayEdit } from "./ArraySettings";
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
  const failure = status?.failure ?? null;
  return (
    <NodeShell
      node={node}
      title="Array"
      category="tool"
      subtitle={arraySubtitle(status, rows.length)}
    >
      <FaceBody>
        <ArrayDial node={node.id} status={status} lead={members[0]} />
        <ArrayChips
          data={node.data}
          status={status}
          members={members}
          memberCount={memberNodes.length}
          positionWired={hasWire(graph, node.id, "position")}
          span={arraySpanHz(graph, workspace.devices, node.id, status)}
          edit={edit}
        />
        {status !== undefined && (
          <ArrayStatusReadout status={status} orientation={node.data.orientation} now={now} />
        )}
        {failure !== null && (
          <FaceFault message={failureText(failure)} detail={failureTitle(failure)} />
        )}
        <ArrayLanes node={node.id} rows={rows} status={status} />
        <ArraySettings data={node.data} status={status} lanes={lanes} edit={edit} />
      </FaceBody>
      <ArrayFooter
        node={node.id}
        status={status}
        aliased={aliasing(node.data.geometry, lanes, status?.center_hz ?? null)}
        lanes={rows.length}
      />
    </NodeShell>
  );
}

function ArrayFooter({
  node,
  status,
  aliased,
  lanes,
}: {
  node: string;
  status: ArrayStatus | undefined;
  aliased: boolean;
  lanes: number;
}) {
  const calibrate = useArrayAction(node, CALIBRATE_ACTION, () => calibrateArray(node));
  const record = useArrayAction(node, RECORD_ACTION, () => startArrayRecording(node));
  const stop = useArrayAction(node, RECORD_ACTION, () => stopArrayRecording(node));
  const recording = status?.recording ?? null;
  const uncalibrated = status === undefined || status.cal === "none" || status.cal === "failed";
  return (
    <FaceFooter>
      <FaceStats>
        {status !== undefined && <ArrayHealth status={status} />}
        {aliased && <Stat label={ALIASING} title={ALIASING_TITLE} tone="danger" />}
      </FaceStats>
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
        disabled={calibrate.isPending || lanes === 0}
        onClick={() => calibrate.mutate()}
      >
        Calibrate
      </Button>
    </FaceFooter>
  );
}

function ArrayHealth({ status }: { status: ArrayStatus }) {
  const gaps = laneGaps(status);
  const recording = status.recording ?? null;
  return (
    <>
      {recording !== null && (
        <Stat
          label="Rec"
          title={recording.error ?? recording.stem}
          tone={recording.dropped > 0 || recording.error != null ? "danger" : undefined}
        >
          {recordingLabel(recording, status.sample_rate)}
        </Stat>
      )}
      {gaps > 0 && (
        <Stat label="Gaps" title="Samples lost and resynced" tone="warn">
          {formatCount(gaps)}
        </Stat>
      )}
      {status.realigns > 0 && (
        <Stat label="Realigns" title="Lanes lined up again" tone="warn">
          {formatCount(status.realigns)}
        </Stat>
      )}
      {status.dropped_samples > 0 && (
        <Stat label="Drops" title="Samples dropped" tone="warn">
          {formatCount(status.dropped_samples)}
        </Stat>
      )}
      {status.events_lost > 0 && (
        <Stat label="Lost" title="Lane events lost" tone="warn">
          {formatCount(status.events_lost)}
        </Stat>
      )}
    </>
  );
}

function ArrayDial({
  node,
  status,
  lead,
}: {
  node: string;
  status: ArrayStatus | undefined;
  lead: DeviceSet | undefined;
}) {
  const { tuneArray } = useArrayTune();
  const centerHz = useArrayStore((store) => shownCenterHz(store, node));
  const active = useFaceActive();
  const range = lead === undefined ? ANY_FREQUENCY : tuningRange(lead.capabilities);
  const held = status === undefined;
  const tune = (hz: number): void => tuneArray(node, hz);
  return (
    <div className="@container flex min-w-0 items-center gap-2 p-2">
      <FrequencyDial
        id={dialId(node, 0)}
        hz={centerHz ?? 0}
        range={range}
        disabled={held}
        wheelTunes={active}
        onTune={tune}
      />
      <span className="ml-auto flex shrink-0 items-center gap-1">
        <TuneTo
          title="Type a frequency"
          hz={centerHz ?? 0}
          hint={`Reaches ${formatMhz(range.min)} to ${formatMhz(range.max)}`}
          resolve={(entered) => inTuningRange(entered, range)}
          disabled={held}
          onTune={tune}
        />
      </span>
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
  return (
    <Readouts columns={2}>
      <Readout label="Sync" title="Lanes lined up in time">
        {syncLabel(status)}
      </Readout>
      <Readout label="Tier" title="Capped: measured drift allows less than declared">
        {tierLabel(status)}
      </Readout>
      <Readout
        label="Cal"
        title={calTitle(status)}
        tone={status.cal === "failed" ? "danger" : undefined}
      >
        {calLabel(status, now)}
      </Readout>
      <Readout label="Heading" title="Array forward, true north">
        {headingLabel(orientation, status)}
      </Readout>
    </Readouts>
  );
}
