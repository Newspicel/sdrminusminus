import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  AUDIO_RECORDINGS_KEY,
  annotateRecording,
  audioRecordingDownloadUrl,
  audioRecordingsQuery,
  deleteAudioRecording,
  deleteRecording,
  RECORDINGS_KEY,
  recordingDownloadUrl,
  recordingsQuery,
} from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RecordingAnnotation, RecordingInfo } from "../lib/types";
import { Button, Form, Input, Textarea } from "./BaseControls";
import { BTN, BTN_SM, CHIP, FIELD } from "./controls";
import {
  describeRecording,
  downloadFormats,
  formatBytes,
  formatDuration,
  formatTags,
  MAX_RECORDING_NAME_LEN,
  matchesRecordingSearch,
  parseTags,
  recordingProvenance,
  recordingTitle,
} from "./recordings";

export function RecordingsPanel({ onOpen }: { onOpen: (recording: RecordingInfo) => void }) {
  const queryClient = useQueryClient();
  const recordings = useQuery(recordingsQuery());
  const [search, setSearch] = useState("");
  const [editing, setEditing] = useState<number | null>(null);

  const invalidate = (): void => {
    void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
  };
  const deleteMut = useMutation({
    mutationFn: deleteRecording,
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });
  const annotateMut = useMutation({
    mutationFn: ({ id, annotation }: { id: number; annotation: RecordingAnnotation }) =>
      annotateRecording(id, annotation),
    onSuccess: () => setEditing(null),
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const listed = recordings.data?.recordings ?? [];
  const shown = listed.filter((r) => matchesRecordingSearch(r, search));

  return (
    <div className="flex flex-col gap-2 p-3">
      {listed.length > 0 && (
        <Input
          className={FIELD}
          type="search"
          name="recording-library-filter"
          placeholder="Search name, tag or note"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Search the recording library"
        />
      )}
      {shown.map((r) => (
        <div key={r.id} className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <div className="min-w-0 flex-1">
              <div className="truncate font-mono text-ink text-sm">{recordingTitle(r)}</div>
              <div className="truncate font-mono text-[10px] text-ink-dim tabular-nums">
                {describeRecording(r)}
              </div>
              <div className="truncate font-mono text-[10px] text-ink-faint">
                {recordingProvenance(r)}
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
              className={BTN}
              aria-expanded={editing === r.id}
              title="A name, tags and a note, kept in the recording's own metadata"
              onClick={() => setEditing(editing === r.id ? null : r.id)}
            >
              Annotate
            </Button>
            <Button
              type="button"
              className={`${BTN} hover:border-danger hover:text-danger`}
              disabled={deleteMut.isPending}
              onClick={() => deleteMut.mutate(r.id)}
            >
              Delete
            </Button>
          </div>
          {editing === r.id ? (
            <AnnotationForm
              recording={r}
              pending={annotateMut.isPending}
              onCancel={() => setEditing(null)}
              onSave={(annotation) => annotateMut.mutate({ id: r.id, annotation })}
            />
          ) : (
            <Annotation recording={r} onPickTag={setSearch} />
          )}
        </div>
      ))}
      {listed.length === 0 && <span className="text-sm text-ink-dim">No recordings yet.</span>}
      {listed.length > 0 && shown.length === 0 && (
        <span className="text-sm text-ink-dim">No recording matches “{search}”.</span>
      )}
      <AudioRecordings />
    </div>
  );
}

function Annotation({
  recording,
  onPickTag,
}: {
  recording: RecordingInfo;
  onPickTag: (tag: string) => void;
}) {
  const tags = recording.tags ?? [];
  if (tags.length === 0 && (recording.note ?? "") === "") {
    return null;
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tags.map((tag) => (
        <Button
          key={tag}
          type="button"
          className={`${CHIP} h-5 px-1.5 text-[10px] hover:border-accent-dim`}
          title={`Search for ${tag}`}
          onClick={() => onPickTag(tag)}
        >
          {tag}
        </Button>
      ))}
      {recording.note != null && recording.note !== "" && (
        <span className="min-w-0 flex-1 truncate text-[11px] text-ink-dim" title={recording.note}>
          {recording.note}
        </span>
      )}
    </div>
  );
}

function AnnotationForm({
  recording,
  pending,
  onSave,
  onCancel,
}: {
  recording: RecordingInfo;
  pending: boolean;
  onSave: (annotation: RecordingAnnotation) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(recording.name ?? "");
  const [tags, setTags] = useState(() => formatTags(recording.tags ?? []));
  const [note, setNote] = useState(recording.note ?? "");

  return (
    <Form
      className="flex flex-col gap-1.5 border-line border-l-2 pl-2"
      onSubmit={(e) => {
        e.preventDefault();
        onSave({
          name: name.trim() === "" ? null : name.trim().slice(0, MAX_RECORDING_NAME_LEN),
          tags: parseTags(tags),
          note: note.trim() === "" ? null : note.trim(),
        });
      }}
    >
      <Input
        className={FIELD}
        placeholder="Name this recording"
        maxLength={MAX_RECORDING_NAME_LEN}
        value={name}
        onChange={(e) => setName(e.target.value)}
        aria-label={`Name for ${recording.file}`}
      />
      <Input
        className={FIELD}
        placeholder="Tags, comma separated"
        value={tags}
        onChange={(e) => setTags(e.target.value)}
        aria-label={`Tags for ${recording.file}`}
      />
      <Textarea
        className={`${FIELD} h-auto min-h-14 resize-y py-1 leading-snug`}
        placeholder="What was on the air, and what to remember about it"
        value={note}
        onChange={(e) => setNote(e.target.value)}
        aria-label={`Note for ${recording.file}`}
      />
      <div className="flex gap-2">
        <Button type="submit" className={BTN_SM} disabled={pending}>
          Save
        </Button>
        <Button type="button" className={BTN_SM} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </Form>
  );
}

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
