// Presets, templates, bookmarks, the band search and the recordings: the things a workspace is
// configured *from*, which are not nodes with a stream to carry, in one drawer rather than as
// node kinds that could never be wired to anything.
//
// Nothing here states a target radio at rest. Templates are the one section that reconfigures one
// radio wholesale, so its cards name the radio on the button that does it; presets cover the
// whole workspace (`PresetSnapshot`); bookmarks and the band search tune whatever the operator
// has selected, and only say so when there is nothing to tune.
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { BandsPanel } from "../components/BandsPanel";
import { BookmarksPanel } from "../components/BookmarksPanel";
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

export function Library() {
  const workspace = useWorkspaceContext();
  const placeNode = useNodePlacement();
  const selected =
    workspace.selected === null ? null : (workspace.devices.get(workspace.selected) ?? null);
  const drawn = [...workspace.devices.values()];
  const only = drawn.length === 1 ? (drawn[0] ?? null) : null;
  const active = selected ?? only;

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
    <Tabs defaultValue="templates" className="flex flex-col gap-0 overflow-hidden rounded-md">
      {/* A header band, like a node's: the sections stay put while their content scrolls under
          them, and the drawer reads as one instrument rather than a list that starts with five
          loose buttons. */}
      <TabsList
        variant="line"
        className="flex h-10 w-full shrink-0 items-center justify-start gap-0.5 border-b border-border bg-muted px-2 py-1.5"
        aria-label="Library section"
      >
        {TABS.map((entry) => (
          <TabsTrigger key={entry.id} value={entry.id}>
            {entry.label}
          </TabsTrigger>
        ))}
      </TabsList>
      <TabsContent value="templates" className="max-h-[28rem] overflow-y-auto">
        <TemplatesPanel active={active} onApplied={() => workspace.apply()} />
      </TabsContent>
      <TabsContent value="presets" className="max-h-[28rem] overflow-y-auto">
        <PresetsPanel />
      </TabsContent>
      <TabsContent value="bookmarks" className="max-h-[28rem] overflow-y-auto">
        <BookmarksPanel active={active} />
      </TabsContent>
      <TabsContent value="bands" className="max-h-[28rem] overflow-y-auto">
        <BandsPanel active={active} />
      </TabsContent>
      <TabsContent value="recordings" className="max-h-[28rem] overflow-y-auto">
        <RecordingsPanel onOpen={openRecording} />
      </TabsContent>
    </Tabs>
  );
}
