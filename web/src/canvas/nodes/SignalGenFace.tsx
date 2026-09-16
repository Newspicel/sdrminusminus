import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN_PRIMARY, BTN_QUIET } from "../../components/controls";
import { inTuningRange, tuningRange } from "../../components/dial";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { formatMhz } from "../../components/format";
import { RadioSettings } from "../../components/RadioSettings";
import { TuneTo } from "../../components/TuneTo";
import { STATE_KEY } from "../../lib/api";
import { toastError } from "../../lib/toasts";
import type { PatchNode, PatchNodeOf } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { releaseRadio } from "../remove";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

type SignalGenNodeData = PatchNodeOf<"signal_gen">["data"];

export function SignalGenFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const queryClient = useQueryClient();
  const { applyPatch } = useDevicePatch();
  const active = useFaceActive();
  const set = workspace.devices.get(node.id) ?? null;

  const editNode = (next: Partial<SignalGenNodeData>): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (stored) =>
        stored.kind === "signal_gen" ? { ...stored, data: { ...stored.data, ...next } } : stored,
      ),
    }));

  const start = (): void => {
    editNode({ running: true });
    workspace.apply();
  };

  const stop = useMutation({
    mutationFn: () => releaseRadio(workspace, node.id, () => editNode({ running: false })),
    onError: (error: Error) => toastError(error),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  if (set === null) {
    return (
      <NodeShell node={node} title="Signal generator" category="source" subtitle="stopped">
        <FaceFooter>
          <Button
            type="button"
            className={BTN_PRIMARY}
            title="Generate a decodable test signal, so a decoder can be tried without a radio"
            onClick={start}
          >
            Start
          </Button>
        </FaceFooter>
      </NodeShell>
    );
  }

  const range = tuningRange(set.capabilities);
  const centerHz = set.settings.center_hz ?? 0;

  return (
    <NodeShell
      node={node}
      title="Signal generator"
      category="source"
      subtitle={<span className={set.status === "error" ? "text-danger" : ""}>{set.status}</span>}
    >
      <FaceBody>
        <div className="@container flex flex-col gap-1 border-b border-line p-2">
          <div className="flex min-w-0 items-center gap-1">
            <FrequencyDial
              id={dialId(node.id, 0)}
              hz={centerHz}
              range={range}
              wheelTunes={active}
              onTune={(hz) => applyPatch(set.id, { center_hz: hz })}
            />
            <span className="ml-auto flex shrink-0 items-center gap-1">
              <TuneTo
                title="Type the frequency to generate at"
                hz={centerHz}
                hint={`Reaches ${formatMhz(range.min)} – ${formatMhz(range.max)}`}
                resolve={(entered) => inTuningRange(entered, range)}
                onTune={(hz) => applyPatch(set.id, { center_hz: hz })}
              />
            </span>
          </div>
        </div>

        <RadioSettings active={set} className="p-2" />

        {set.error != null && (
          <p role="alert" className="border-t border-line p-2 font-mono text-xs text-danger">
            {set.error}
          </p>
        )}
      </FaceBody>
      <FaceFooter>
        <Button
          type="button"
          className={BTN_QUIET}
          title="Stop generating and free the node — the wires stay drawn"
          onClick={() => stop.mutate()}
          disabled={stop.isPending}
        >
          {stop.isPending ? "Stopping…" : "Stop"}
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}
