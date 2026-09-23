import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, FolderOpen, Pencil, Trash2 } from "lucide-react";
import { useState } from "react";
import {
  AUDIO_RECORDINGS_KEY,
  aboutQuery,
  annotateRecording,
  audioRecordingDownloadUrl,
  audioRecordingsQuery,
  audioRecordingUrl,
  deleteAudioRecording,
  deleteRecording,
  RECORDINGS_KEY,
  recordingDownloadUrl,
  recordingsQuery,
  revealAudioRecording,
  revealRecording,
  revealRecordingsDir,
} from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RecordingAnnotation, RecordingInfo } from "../lib/types";
import { Button, Form, Input, Textarea } from "./BaseControls";
import { BTN_SM, CHIP_SM, FIELD } from "./controls";
import { formatBytes, formatSampleRate } from "./format";
import {
  List,
  ListRow,
  Panel,
  PanelHint,
  PanelToolbar,
  RowAction,
  RowLink,
  SearchField,
} from "./ListPanel";
import { RecordingUpload } from "./RecordingUpload";
import {
  describeRecording,
  downloadFormats,
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
  const reveal = useQuery(aboutQuery(true)).data?.reveal === true;
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

  const revealMut = useMutation({
    mutationFn: revealRecording,
    onError: (e) => pushToast(e.message),
  });

  const listed = recordings.data?.recordings ?? [];
  const shown = listed.filter((r) => matchesRecordingSearch(r, search));
  const dir = recordings.data?.dir;

  return (
    <Panel>
      <PanelToolbar>
        {listed.length > 0 && (
          <SearchField
            type="search"
            name="recording-library-filter"
            placeholder="Search name, tag or note"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            aria-label="Search the recording library"
          />
        )}
        <RecordingUpload compact={listed.length > 0} />
      </PanelToolbar>
      {dir != null && <RecordingsFolder dir={dir} reveal={reveal} />}
      {listed.length === 0 && <PanelHint>No recordings yet.</PanelHint>}
      {listed.length > 0 && shown.length === 0 && (
        <PanelHint>No recording matches “{search}”.</PanelHint>
      )}
      {shown.length > 0 && (
        <List title="IQ">
          {shown.map((r) => (
            <ListRow
              key={r.id}
              primary={recordingTitle(r)}
              secondary={describeRecording(r)}
              hint={recordingProvenance(r)}
              actions={
                <>
                  <Button type="button" className={BTN_SM} onClick={() => onOpen(r)}>
                    Open as source
                  </Button>
                  {downloadFormats.map(({ format, label, hint }) => (
                    <a
                      key={format}
                      className={BTN_SM}
                      href={recordingDownloadUrl(r.id, format)}
                      title={hint}
                      download
                    >
                      {label}
                    </a>
                  ))}
                  <RowAction
                    label={`Annotate ${recordingTitle(r)}`}
                    glyph={Pencil}
                    onClick={() => setEditing(editing === r.id ? null : r.id)}
                  />
                  {reveal && (
                    <RowAction
                      label="Show in folder"
                      glyph={FolderOpen}
                      onClick={() => revealMut.mutate(r.id)}
                    />
                  )}
                  <RowAction
                    label={`Delete ${recordingTitle(r)}`}
                    glyph={Trash2}
                    danger
                    disabled={deleteMut.isPending}
                    onClick={() => deleteMut.mutate(r.id)}
                  />
                </>
              }
            >
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
            </ListRow>
          ))}
        </List>
      )}
      <AudioRecordings reveal={reveal} />
    </Panel>
  );
}

function RecordingsFolder({ dir, reveal }: { dir: string; reveal: boolean }) {
  const revealMut = useMutation({
    mutationFn: revealRecordingsDir,
    onError: (e) => pushToast(e.message),
  });
  return (
    <PanelToolbar>
      <span className="legend min-w-0 flex-1 truncate" title={dir}>
        {dir}
      </span>
      {reveal && (
        <Button
          type="button"
          className={BTN_SM}
          title="Open this folder in the machine's file manager"
          onClick={() => revealMut.mutate()}
        >
          Show in folder
        </Button>
      )}
    </PanelToolbar>
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
          className={`${CHIP_SM} hover:border-accent-dim hover:text-accent`}
          title={`Search for ${tag}`}
          onClick={() => onPickTag(tag)}
        >
          {tag}
        </Button>
      ))}
      {recording.note != null && recording.note !== "" && (
        <span className="min-w-0 flex-1 truncate text-xs text-ink-dim" title={recording.note}>
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
      className="flex flex-col gap-1.5 border-accent-dim border-l-2 pl-2"
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

function AudioRecordings({ reveal }: { reveal: boolean }) {
  const queryClient = useQueryClient();
  const recordings = useQuery(audioRecordingsQuery());
  const deleteMut = useMutation({
    mutationFn: deleteAudioRecording,
    onError: (e) => pushToast(e.message),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: AUDIO_RECORDINGS_KEY });
    },
  });
  const revealMut = useMutation({
    mutationFn: revealAudioRecording,
    onError: (e) => pushToast(e.message),
  });
  const listed = recordings.data?.recordings ?? [];
  if (listed.length === 0) {
    return null;
  }
  return (
    <List title="Channel audio">
      {listed.map((r) => (
        <ListRow
          key={r.file}
          primary={r.file}
          secondary={`${r.channels === 2 ? "stereo" : "mono"} · ${formatSampleRate(r.sample_rate)} · ${formatDuration(r.duration_s)} · ${formatBytes(r.bytes)}`}
          actions={
            <>
              {reveal && (
                <RowAction
                  label="Show in folder"
                  glyph={FolderOpen}
                  onClick={() => revealMut.mutate(r.file)}
                />
              )}
              <RowLink
                label="Download WAV"
                glyph={Download}
                href={audioRecordingDownloadUrl(r.file)}
              />
              <RowAction
                label={`Delete ${r.file}`}
                glyph={Trash2}
                danger
                disabled={deleteMut.isPending}
                onClick={() => deleteMut.mutate(r.file)}
              />
            </>
          }
        >
          <audio
            className="h-8 w-full min-w-0"
            controls
            preload="none"
            src={audioRecordingUrl(r.file)}
          />
        </ListRow>
      ))}
    </List>
  );
}
