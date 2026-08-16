import { useMutation } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { formatBytes, formatDuration } from "../../components/recordings";
import { SettingRow, Settings } from "../../components/Settings";
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
import { hasWire, iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { RADIO_IDLE } from "./faceCopy";
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
  const stream = iqSourceOf(workspace.graph, node.id)?.stream ?? 0;
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
    <NodeShell
      node={node}
      title="Time machine"
      category="sink"
      subtitle={
        phase.kind === "capturing"
          ? "capturing"
          : phase.kind === "armed"
            ? `holding ${seconds} s`
            : undefined
      }
      live={phase.kind === "armed" || phase.kind === "capturing"}
    >
      <FaceBody>
        {set !== null && (
          <Settings className="border-b border-line p-2">
            <SettingRow label="History">
              <NumberField
                label="Seconds of history"
                value={seconds}
                min={MIN_HISTORY_SECONDS}
                max={MAX_HISTORY_SECONDS}
                step={1}
                disabled={phase.kind !== "idle" || control.isPending}
                onCommit={edit}
                className="w-24"
              />
              <span className="legend">s</span>
            </SettingRow>
          </Settings>
        )}
        {status === null ? (
          <FaceEmpty>
            {phase.kind !== "unavailable"
              ? `Arm it and the last ${seconds} s stay in memory, ready to be written after the fact.`
              : hasWire(workspace.graph, node.id, "iq")
                ? RADIO_IDLE
                : "Wire a device's IQ in; the buffer holds what it has already heard."}
          </FaceEmpty>
        ) : (
          <HistoryReadout status={status} />
        )}
        {status?.error != null && (
          <p role="alert" className="border-t border-line p-2 text-xs text-danger">
            {status.error}
          </p>
        )}
      </FaceBody>
      {set !== null && (
        <FaceFooter>
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
                  <span aria-hidden className="text-danger">
                    ●
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
      )}
    </NodeShell>
  );
}

function HistoryReadout({ status }: { status: TimeMachineStatus }) {
  const capture = status.capture ?? null;
  return (
    <Readout separated={false}>
      <ReadoutRow label="Held">
        {formatDuration(heldSeconds(status))} · {(historyFill(status) * 100).toFixed(0)}% of{" "}
        {status.history_seconds} s
      </ReadoutRow>
      <ReadoutRow label="Memory">
        {formatBytes(status.capacity_samples * HISTORY_BYTES_PER_SAMPLE)}
      </ReadoutRow>
      {status.overruns > 0 && <ReadoutRow label="Drops">{status.overruns}</ReadoutRow>}
      {capture !== null && (
        <>
          <ReadoutRow label="Written">{formatBytes(capture.bytes)}</ReadoutRow>
          <ReadoutRow label="File">
            <span className="block truncate" title={capture.file}>
              {capture.file}
            </span>
          </ReadoutRow>
        </>
      )}
    </Readout>
  );
}
