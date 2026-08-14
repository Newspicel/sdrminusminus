// SigMF recordings browser (: files on disk are the source of truth; the list is the
// reconciled index, WS-invalidated on scope "recordings"). A recording plays back through a
// `virtual:file:` device, so opening one is the same gesture as opening a radio: it draws a
// source node on the canvas, and apply is what starts it.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemGroup,
  ItemTitle,
} from "@/components/ui/item";
import { deleteRecording, RECORDINGS_KEY, recordingDownloadUrl, recordingsQuery } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RecordingInfo } from "../lib/types";
import { EmptyState } from "./EmptyState";
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
      <ItemGroup>
        {(recordings.data?.recordings ?? []).map((r) => (
          <Item key={r.id} size="xs">
            <ItemContent>
              <ItemTitle className="font-mono">{r.file}</ItemTitle>
              <ItemDescription className="font-mono text-[10px] tabular-nums">
                {formatMhz(r.center_hz)} · {(r.sample_rate / 1e6).toFixed(3)} MS/s ·{" "}
                {formatDuration(r.duration_s)} · {formatBytes(r.bytes)}
              </ItemDescription>
            </ItemContent>
            <ItemActions>
              <ButtonGroup>
                <Button type="button" variant="outline" size="sm" onClick={() => onOpen(r)}>
                  Open as source
                </Button>
                {downloadFormats.map(({ format, label, hint }) => (
                  <Button
                    key={format}
                    render={<a href={recordingDownloadUrl(r.id, format)} download />}
                    variant="outline"
                    size="sm"
                    title={hint}
                  >
                    {label}
                  </Button>
                ))}
                <Button
                  type="button"
                  variant="destructive"
                  size="sm"
                  disabled={deleteMut.isPending}
                  onClick={() => deleteMut.mutate(r.id)}
                >
                  Delete
                </Button>
              </ButtonGroup>
            </ItemActions>
          </Item>
        ))}
      </ItemGroup>
      {recordings.data?.recordings.length === 0 && <EmptyState>No recordings yet.</EmptyState>}
    </div>
  );
}
