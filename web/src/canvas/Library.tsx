// Presets, templates, bookmarks, the band search and the recordings: the things a workspace is
// configured *from*, which are not nodes with a stream to carry, in one drawer rather than as
// node kinds that could never be wired to anything.
//
// Nothing here states a target radio at rest. Templates are the one section that reconfigures one
// radio wholesale, so its cards name the radio on the button that does it; presets cover the
// whole workspace (`PresetSnapshot`); bookmarks and the band search tune whatever the operator
// has selected, and only say so when there is nothing to tune.
import { useState } from "react";
import { BandsPanel } from "../components/BandsPanel";
import { BookmarksPanel } from "../components/BookmarksPanel";
import { segment } from "../components/controls";
import { PresetsPanel } from "../components/PresetsPanel";
import { RecordingsPanel } from "../components/RecordingsPanel";
import { TemplatesPanel } from "../components/TemplatesPanel";
import { pushToast } from "../lib/toasts";
import type { RecordingInfo } from "../lib/types";
import { refFromDeviceId } from "./binding";
import { useWorkspaceContext } from "./context";
import { addNode, MAX_NAME_LEN, newNodeId } from "./graph";
import { useNodePlacement } from "./placement";

const TABS = [
  { id: "templates", label: "Templates" },
  { id: "presets", label: "Presets" },
  { id: "bookmarks", label: "Bookmarks" },
  { id: "bands", label: "Bands" },
  { id: "recordings", label: "Recordings" },
] as const;

type Tab = (typeof TABS)[number]["id"];

export function Library() {
  const workspace = useWorkspaceContext();
  const placeNode = useNodePlacement();
  const [tab, setTab] = useState<Tab>("templates");
  // The radio a section that acts on *one* of them means: the selected device node, falling back
  // to the only one drawn. Applying a template to the wrong radio is not recoverable by undo, so
  // with several drawn and none selected there is no target at all — the section says so rather
  // than silently picking the first.
  //
  // "One radio to mean" counts the radios *this patch names*, not every set the engine has open:
  // applying a patch never closes anything (CANVAS §4), so a workspace with nothing drawn on it
  // still sits beside whatever the last one left running.
  const selected =
    workspace.selected === null ? null : (workspace.devices.get(workspace.selected) ?? null);
  const drawn = [...workspace.devices.values()];
  const only = drawn.length === 1 ? (drawn[0] ?? null) : null;
  const active = selected ?? only;

  /** Draw a recording onto the canvas as the source it already is: a device node bound to the
   * `virtual:file:` playback device. Apply is what opens it, so a recording that has since been
   * deleted lands in the apply report's `absent` list and renders as a disconnected node —
   * CANVAS §3's bound-but-absent, which is the honest state for a file that is not there. */
  const openRecording = (recording: RecordingInfo): void => {
    const device = refFromDeviceId(recording.device_id);
    if (device === null) {
      pushToast(`${recording.file} has no playable device id`);
      return;
    }
    const id = newNodeId("device");
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: addNode(snapshot.graph, {
        id,
        kind: "device",
        data: { device },
        position: placeNode(snapshot.graph, "device"),
        // Bounded like every other node label: the server validates the whole snapshot on every
        // write, so one over-long label would refuse the next node drag too.
        label: recording.file.slice(0, MAX_NAME_LEN),
      }),
    }));
    workspace.select(id);
    workspace.apply();
  };

  return (
    <div className="flex flex-col overflow-hidden rounded-md">
      {/* A header band, like a node's: the sections stay put while their content scrolls under
          them, and the drawer reads as one instrument rather than a list that starts with five
          loose buttons. */}
      <div
        className="flex shrink-0 items-center gap-0.5 border-b border-line bg-panel-2 px-2 py-1.5"
        role="group"
        aria-label="Library section"
      >
        {TABS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            className={segment(tab === entry.id)}
            aria-pressed={tab === entry.id}
            onClick={() => setTab(entry.id)}
          >
            {entry.label}
          </button>
        ))}
      </div>
      <div className="max-h-[28rem] overflow-y-auto">
        {tab === "templates" && (
          <TemplatesPanel active={active} onApplied={() => workspace.apply()} />
        )}
        {tab === "presets" && <PresetsPanel />}
        {tab === "bookmarks" && <BookmarksPanel active={active} />}
        {tab === "bands" && <BandsPanel active={active} />}
        {tab === "recordings" && <RecordingsPanel onOpen={openRecording} />}
      </div>
    </div>
  );
}
