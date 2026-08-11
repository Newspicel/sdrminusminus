// SigMF recordings browser (PLAN §11: files on disk are the source of truth; the list is the
// reconciled index, WS-invalidated on scope "recordings"). A recording plays back through a
// `virtual:file:` device, so opening one is the same gesture as opening a radio: it draws a
// source node on the canvas, and apply is what starts it.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { deleteRecording, RECORDINGS_KEY, recordingsQuery } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RecordingInfo } from "../lib/types";
import { BTN } from "./controls";
import { formatMhz } from "./format";
import { formatBytes, formatDuration } from "./recordings";

export function RecordingsPanel({ onOpen }: { onOpen: (recording: RecordingInfo) => void }) {
  const queryClient = useQueryClient();
  const recordings = useQuery(recordingsQuery());

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
  };
  const deleteMut = useMutation({
    mutationFn: deleteRecording,
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  return (
    <div className="flex flex-col gap-2 p-3">
      {(recordings.data?.recordings ?? []).map((r) => (
        <div key={r.id} className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate font-mono text-sm text-ink">{r.file}</div>
            <div className="truncate font-mono text-[10px] tabular-nums text-ink-dim">
              {formatMhz(r.center_hz)} · {(r.sample_rate / 1e6).toFixed(3)} MS/s ·{" "}
              {formatDuration(r.duration_s)} · {formatBytes(r.bytes)}
            </div>
          </div>
          <button type="button" className={BTN} onClick={() => onOpen(r)}>
            Open as source
          </button>
          <button
            type="button"
            className={`${BTN} hover:border-danger hover:text-danger`}
            disabled={deleteMut.isPending}
            onClick={() => deleteMut.mutate(r.id)}
          >
            Delete
          </button>
        </div>
      ))}
      {recordings.data?.recordings.length === 0 && (
        <span className="text-sm text-ink-dim">No recordings yet.</span>
      )}
    </div>
  );
}
