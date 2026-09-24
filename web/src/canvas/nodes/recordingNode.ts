import { matchesRecordingSearch, recordingTitle } from "../../components/recordings";
import type { PatchNodeOf, Position, RecordingInfo } from "../../lib/types";
import { MAX_NAME_LEN } from "../graph";

export const META_SUFFIX = ".sigmf-meta";
export const DATA_SUFFIX = ".sigmf-data";
export const ARCHIVE_SUFFIX = ".sigmf";

export const UPLOAD_ACCEPT = [ARCHIVE_SUFFIX, META_SUFFIX, DATA_SUFFIX].join(",");

export function recordingDeviceId(stem: string): string {
  return `recording:${stem}`;
}

export function findRecording(
  library: readonly RecordingInfo[],
  stem: string | null | undefined,
): RecordingInfo | null {
  if (stem == null || stem === "") {
    return null;
  }
  return library.find((recording) => recording.file === stem) ?? null;
}

export function claimedRecordings(
  graph: { nodes: readonly { id: string; kind: string; data?: unknown }[] },
  exceptNode: string,
): string[] {
  const claimed: string[] = [];
  for (const node of graph.nodes) {
    if (node.kind !== "recording" || node.id === exceptNode) {
      continue;
    }
    const stem = (node.data as { recording?: string | null } | undefined)?.recording;
    if (stem != null && stem !== "") {
      claimed.push(stem);
    }
  }
  return claimed;
}

export function recordingChoices(
  library: readonly RecordingInfo[],
  claimed: readonly string[],
  search: string,
): readonly RecordingInfo[] {
  return library
    .filter((recording) => !claimed.includes(recording.file))
    .filter((recording) => matchesRecordingSearch(recording, search))
    .toSorted((a, b) => recordingTitle(a).localeCompare(recordingTitle(b)));
}

export type UploadProblem = "empty" | "lone-meta" | "lone-data" | "mixed";

export function checkUpload(names: readonly string[]): UploadProblem | null {
  if (names.length === 0) {
    return "empty";
  }
  const meta = names.filter((name) => name.endsWith(META_SUFFIX)).length;
  const data = names.filter((name) => name.endsWith(DATA_SUFFIX)).length;
  const archives = names.length - meta - data;
  if (archives > 0) {
    return archives === names.length && archives === 1 ? null : "mixed";
  }
  if (meta === 1 && data === 1) {
    return null;
  }
  return data === 0 ? "lone-meta" : "lone-data";
}

export const UPLOAD_SAID: Record<UploadProblem, string> = {
  empty: "Pick a .sigmf archive, or a .sigmf-meta and .sigmf-data pair.",
  "lone-meta": "That is the metadata on its own: add the matching .sigmf-data.",
  "lone-data": "That is the samples on their own: add the matching .sigmf-meta.",
  mixed: "Send one .sigmf archive, or one .sigmf-meta with one .sigmf-data.",
};

export function recordingNodeFor(
  recording: RecordingInfo,
  id: string,
  position: Position,
): PatchNodeOf<"recording"> {
  return {
    id,
    kind: "recording",
    data: { recording: recording.file },
    position,
    label: recordingTitle(recording).slice(0, MAX_NAME_LEN),
  };
}
