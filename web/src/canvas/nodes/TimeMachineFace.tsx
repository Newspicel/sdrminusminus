import { useMutation } from "@tanstack/react-query";
import { Circle } from "lucide-react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER } from "../../components/controls";
import { Chips, NumberChip } from "../../components/face/Chips";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { FaceStats, Stat } from "../../components/face/Stats";
import { DROPS_HINT, formatBytes, formatCount } from "../../components/format";
import { Icon } from "../../components/Icon";
import { formatDuration } from "../../components/recordings";
import {
  DEFAULT_HISTORY_SECONDS,
  HISTORY_BYTES_PER_SAMPLE,
  heldSeconds,
  historyFill,
  MAX_HISTORY_SECONDS,
  MIN_HISTORY_SECONDS,
  timeMachineMutationOptions,
  timeMachinePhase,
} from "../../components/timeMachine";
import type { PatchNode, PatchNodeOf, TimeMachineStatus } from "../../lib/types";
import { iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { FaceBody, FaceEmpty, FaceFooter, NodeShell } from "./NodeShell";

export function TimeMachineFace({ node }: { node: PatchNode }) {
  if (node.kind !== "time_machine") {
    return null;
  }
  return <TimeMachineNodeFace node={node} />;
}

function TimeMachineNodeFace({ node }: { node: PatchNodeOf<"time_machine"> }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const stream = iqSourceOf(workspace.graph, node.id, workspace.devices)?.stream ?? 0;
  const seconds = node.data.history_seconds ?? DEFAULT_HISTORY_SECONDS;
  const phase = timeMachinePhase(set, node.id);
  const status = phase.kind === "armed" || phase.kind === "capturing" ? phase.status : null;
  const control = useMutation(
    timeMachineMutationOptions(set === null ? null : set.id, node.id, stream, {
      history_seconds: seconds,
    }),
  );

  const edit = (history_seconds: number) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "time_machine"
          ? { ...current, data: { ...current.data, history_seconds } }
          : current,
      ),
    }));
  };

  return (
    <NodeShell node={node} title="Time machine" category="output">
      <FaceBody>
        <Chips className="p-2">
          <NumberChip
            label="History"
            title="Seconds of history"
            value={seconds}
            unit="s"
            min={MIN_HISTORY_SECONDS}
            max={MAX_HISTORY_SECONDS}
            step={1}
            disabled={phase.kind !== "idle" || control.isPending}
            onCommit={edit}
          />
        </Chips>
        {status === null ? (
          <FaceEmpty
            hint={
              phase.kind !== "unavailable"
                ? `Arm it to keep the last ${seconds} s in memory`
                : "Wire a device's IQ in"
            }
          />
        ) : (
          <HistoryReadout status={status} />
        )}
        {status?.error != null && <FaceFault message={status.error} />}
      </FaceBody>
      <FaceFooter>
        {status !== null && status.overruns > 0 && (
          <FaceStats>
            <Stat label="Drops" title={DROPS_HINT} tone="warn">
              {formatCount(status.overruns)}
            </Stat>
          </FaceStats>
        )}
        {phase.kind === "idle" || phase.kind === "unavailable" ? (
          <Button
            type="button"
            className={BTN}
            disabled={phase.kind !== "idle" || control.isPending}
            title="Hold the last seconds of IQ in memory"
            onClick={() => control.mutate("arm")}
          >
            Arm
          </Button>
        ) : (
          <>
            {phase.kind === "armed" ? (
              <Button
                type="button"
                className={BTN}
                disabled={control.isPending}
                title="Write the buffered past to a SigMF pair and keep recording"
                onClick={() => control.mutate("capture")}
              >
                <span className="flex text-danger">
                  <Icon glyph={Circle} size={12} filled />
                </span>
                Capture
              </Button>
            ) : (
              <Button
                type="button"
                className={BTN_DANGER}
                disabled={control.isPending}
                onClick={() => control.mutate("stop")}
              >
                Stop
              </Button>
            )}
            <Button
              type="button"
              className={BTN}
              disabled={control.isPending}
              title="Release the buffer"
              onClick={() => control.mutate("disarm")}
            >
              Disarm
            </Button>
          </>
        )}
      </FaceFooter>
    </NodeShell>
  );
}

function HistoryReadout({ status }: { status: TimeMachineStatus }) {
  const capture = status.capture ?? null;
  return (
    <Readouts>
      <Readout label="Held">
        {formatDuration(heldSeconds(status))} · {(historyFill(status) * 100).toFixed(0)}% of{" "}
        {status.history_seconds} s
      </Readout>
      <Readout label="Memory">
        {formatBytes(status.capacity_samples * HISTORY_BYTES_PER_SAMPLE)}
      </Readout>
      {capture !== null && (
        <>
          <Readout label="Written">{formatBytes(capture.bytes)}</Readout>
          <Readout label="File">
            <span className="block truncate" title={capture.file}>
              {capture.file}
            </span>
          </Readout>
        </>
      )}
    </Readouts>
  );
}
