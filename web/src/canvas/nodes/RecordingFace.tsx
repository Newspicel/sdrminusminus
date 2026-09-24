import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN, BTN_PRIMARY, BTN_QUIET, FIELD } from "../../components/controls";
import { deviceId } from "../../components/devices";
import { formatBytes, formatMhz, formatSampleRate } from "../../components/format";
import { PlaybackTransport } from "../../components/PlaybackTransport";
import { Readout, ReadoutRow } from "../../components/Readout";
import { RecordingUpload } from "../../components/RecordingUpload";
import {
  describeRecording,
  formatDuration,
  recordingProvenance,
  recordingTitle,
} from "../../components/recordings";
import { createDeviceSet, recordingsQuery, STATE_KEY } from "../../lib/api";
import { toastError } from "../../lib/toasts";
import type { PatchNode, PatchNodeOf, RecordingInfo } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { releaseRadio } from "../remove";
import { FaceBody, FaceFooter, NodeShell } from "./NodeShell";
import { claimedRecordings, recordingChoices, recordingDeviceId } from "./recordingNode";

type RecordingNodeData = PatchNodeOf<"recording">["data"];

function Library({
  node,
  onPick,
  busy,
}: {
  node: string;
  onPick: (recording: RecordingInfo) => void;
  busy: boolean;
}) {
  const workspace = useWorkspaceContext();
  const library = useQuery(recordingsQuery());
  const [search, setSearch] = useState("");
  const all = library.data?.recordings ?? [];
  const choices = recordingChoices(all, claimedRecordings(workspace.graph, node), search);

  return (
    <div className="flex flex-col gap-2 p-2">
      <div className="flex items-center gap-2">
        {all.length > 0 && (
          <Input
            className={`${FIELD} min-w-0 flex-1`}
            type="search"
            name="recording-filter"
            placeholder="Search recordings"
            aria-label="Search recordings"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        )}
        <RecordingUpload compact={all.length > 0} onUploaded={onPick} />
      </div>
      <div className="flex max-h-64 flex-col gap-1 overflow-y-auto">
        {choices.map((recording) => (
          <Button
            key={recording.id}
            type="button"
            className={`${BTN} h-auto min-h-7 shrink-0 justify-start py-1.5 text-left`}
            title={recordingProvenance(recording)}
            disabled={busy}
            onClick={() => onPick(recording)}
          >
            <span className="flex w-full min-w-0 flex-col gap-0.5">
              <span className="truncate">{recordingTitle(recording)}</span>
              <span className="truncate font-mono text-[10px] text-ink-dim tabular-nums">
                {describeRecording(recording)}
              </span>
            </span>
          </Button>
        ))}
      </div>
      {library.isPending && <p className="text-sm text-ink-dim">Reading the library…</p>}
      {!library.isPending && all.length === 0 && (
        <p className="text-sm text-ink-dim">No recordings yet.</p>
      )}
      {all.length > 0 && choices.length === 0 && (
        <p className="text-sm text-ink-dim">Nothing free matches that.</p>
      )}
    </div>
  );
}

export function RecordingFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const queryClient = useQueryClient();
  const library = useQuery(recordingsQuery());
  const stem = node.kind === "recording" ? (node.data.recording ?? null) : null;
  const set = workspace.devices.get(node.id) ?? null;
  const known =
    stem === null
      ? null
      : (library.data?.recordings.find((recording) => recording.file === stem) ?? null);

  const editNode = (next: Partial<RecordingNodeData>): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (stored) =>
        stored.kind === "recording" ? { ...stored, data: { ...stored.data, ...next } } : stored,
      ),
    }));

  const open = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: () => workspace.apply(),
    onError: (error: Error) => toastError(error),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const forget = useMutation({
    mutationFn: () => releaseRadio(workspace, node.id, () => editNode({ recording: null })),
    onError: (error: Error) => toastError(error),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const pick = (recording: RecordingInfo): void => {
    editNode({ recording: recording.file });
    if (
      workspace.deviceSets.some((candidate) => deviceId(candidate.device) === recording.device_id)
    ) {
      workspace.apply();
    } else {
      open.mutate(recording.device_id);
    }
  };

  if (stem === null) {
    return (
      <NodeShell node={node} title="Recording" category="source">
        <FaceBody>
          <Library node={node.id} onPick={pick} busy={open.isPending} />
        </FaceBody>
      </NodeShell>
    );
  }

  if (set === null) {
    const gone = library.isSuccess && known === null;
    return (
      <NodeShell
        node={node}
        title="Recording"
        category="source"
        subtitle={gone ? "missing" : undefined}
      >
        <FaceBody>
          <p className="p-3 font-mono text-sm text-ink">{known?.file ?? stem}</p>
          {known !== null && (
            <p className="px-3 pb-3 font-mono text-[10px] text-ink-dim tabular-nums">
              {describeRecording(known)}
            </p>
          )}
        </FaceBody>
        <FaceFooter>
          <Button
            type="button"
            className={BTN_QUIET}
            title="Free this node so you can pick another recording"
            onClick={() => forget.mutate()}
            disabled={forget.isPending}
          >
            Forget recording
          </Button>
          <Button
            type="button"
            className={BTN_PRIMARY}
            title={
              gone
                ? "This recording is no longer in the library"
                : "Start playing this recording into whatever is wired to it"
            }
            onClick={() => open.mutate(recordingDeviceId(stem))}
            disabled={gone || open.isPending}
          >
            Play
          </Button>
        </FaceFooter>
      </NodeShell>
    );
  }

  return (
    <NodeShell
      node={node}
      title={known === null ? stem : recordingTitle(known)}
      category="source"
      subtitle={set.status === "error" ? <span className="text-danger">error</span> : undefined}
    >
      <FaceBody>
        {set.playback != null && <PlaybackTransport set={set} status={set.playback} />}
        {known !== null && (
          <Readout separated={set.playback == null}>
            <ReadoutRow label="Centre">{formatMhz(known.center_hz)}</ReadoutRow>
            <ReadoutRow label="Rate">{formatSampleRate(known.sample_rate)}</ReadoutRow>
            <ReadoutRow label="Length">{formatDuration(known.duration_s)}</ReadoutRow>
            <ReadoutRow label="Size">{formatBytes(known.bytes)}</ReadoutRow>
          </Readout>
        )}
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
          title="Stop playing and free the node: the wires stay drawn"
          onClick={() => forget.mutate()}
          disabled={forget.isPending}
        >
          {forget.isPending ? "Closing…" : "Forget recording"}
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}
