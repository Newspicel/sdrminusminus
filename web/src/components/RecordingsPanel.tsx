// SigMF recordings browser (: files on disk are the source of truth; the list is the
// reconciled index, WS-invalidated on scope "recordings"). A recording plays back through a
// `virtual:file:` device, so opening one is the same gesture as opening a radio: it draws a
// source node on the canvas, and apply is what starts it.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AUDIO_RECORDINGS_KEY,
  audioRecordingDownloadUrl,
  audioRecordingsQuery,
  deleteAudioRecording,
  deleteRecording,
  RECORDINGS_KEY,
  recordingDownloadUrl,
  recordingsQuery,
} from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RecordingInfo } from "../lib/types";
import { Button } from "./BaseControls";
import { BTN } from "./controls";
import { formatMhz } from "./format";
import { downloadFormats, formatBytes, formatDuration } from "./recordings";

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
          <Button type="button" className={BTN} onClick={() => onOpen(r)}>
            Open as source
          </Button>
          {downloadFormats.map(({ format, label, hint }) => (
            <a
              key={format}
              className={BTN}
              href={recordingDownloadUrl(r.id, format)}
              title={hint}
              download
            >
              {label}
            </a>
          ))}
          <Button
            type="button"
            className={`${BTN} hover:border-danger hover:text-danger`}
            disabled={deleteMut.isPending}
            onClick={() => deleteMut.mutate(r.id)}
          >
            Delete
          </Button>
        </div>
      ))}
      {recordings.data?.recordings.length === 0 && (
        <span className="text-sm text-ink-dim">No recordings yet.</span>
      )}
      <AudioRecordings />
    </div>
  );
}

/** Channel audio, in the same drawer as the IQ it was demodulated from. No "open as source":
 * a WAV of speech is not a signal a receiver can be pointed at — it is listened to elsewhere. */
function AudioRecordings() {
  const queryClient = useQueryClient();
  const recordings = useQuery(audioRecordingsQuery());
  const deleteMut = useMutation({
    mutationFn: deleteAudioRecording,
    onError: (e) => pushToast(e.message),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: AUDIO_RECORDINGS_KEY });
    },
  });
  const listed = recordings.data?.recordings ?? [];
  if (listed.length === 0) {
    return null;
  }
  return (
    <>
      <div className="mt-1 border-line border-t pt-2 text-xs text-ink-dim">Channel audio</div>
      {listed.map((r) => (
        <div key={r.file} className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate font-mono text-ink text-sm">{r.file}</div>
            <div className="truncate font-mono text-[10px] text-ink-dim tabular-nums">
              {r.channels === 2 ? "stereo" : "mono"} · {(r.sample_rate / 1000).toFixed(1)} kHz ·{" "}
              {formatDuration(r.duration_s)} · {formatBytes(r.bytes)}
            </div>
          </div>
          <a
            className={BTN}
            href={audioRecordingDownloadUrl(r.file)}
            title="16-bit PCM WAV of what the channel sounded like"
            download
          >
            .wav
          </a>
          <Button
            type="button"
            className={`${BTN} hover:border-danger hover:text-danger`}
            disabled={deleteMut.isPending}
            onClick={() => deleteMut.mutate(r.file)}
          >
            Delete
          </Button>
        </div>
      ))}
    </>
  );
}
