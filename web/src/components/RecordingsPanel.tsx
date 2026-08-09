// SigMF recordings browser (PLAN §11: files on disk are the source of truth; the list is the
// reconciled index, WS-invalidated on scope "recordings"). Play opens the pair as a
// `virtual:file:` playback set — the same create-and-select flow as DeviceBar's open buttons.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { createDeviceSet, deleteRecording, RECORDINGS_KEY, recordingsQuery } from "../lib/api";
import { BTN } from "./controls";
import { formatMhz } from "./format";
import { formatBytes, formatDuration } from "./recordings";

export function RecordingsPanel({ onSelect }: { onSelect: (ds: number) => void }) {
  const queryClient = useQueryClient();
  const recordings = useQuery(recordingsQuery());
  const [error, setError] = useState<string | null>(null);

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
  };
  // A failed open must surface here (CLAUDE.md: no silent failure) — the WS state event never
  // fires for a set that was never created.
  const playMut = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: (id) => {
      setError(null);
      onSelect(id);
    },
    onError: (e) => setError(e.message),
  });
  const deleteMut = useMutation({
    mutationFn: deleteRecording,
    onSuccess: () => setError(null),
    onError: (e) => setError(e.message),
    onSettled: invalidate,
  });

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      {error !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Rejected: {error}</span>
          <button type="button" className="shrink-0 underline" onClick={() => setError(null)}>
            dismiss
          </button>
        </div>
      )}

      {(recordings.data?.recordings ?? []).map((r) => (
        <div key={r.id} className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate font-mono text-sm text-ink">{r.file}</div>
            <div className="truncate font-mono text-[10px] tabular-nums text-ink-dim">
              {formatMhz(r.center_hz)} · {(r.sample_rate / 1e6).toFixed(3)} MS/s ·{" "}
              {formatDuration(r.duration_s)} · {formatBytes(r.bytes)}
            </div>
          </div>
          <button
            type="button"
            className={BTN}
            disabled={playMut.isPending}
            onClick={() => playMut.mutate(r.device_id)}
          >
            Play
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
