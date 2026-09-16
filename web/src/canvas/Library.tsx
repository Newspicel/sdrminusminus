import { Tabs } from "@base-ui/react/tabs";
import { BandsPanel } from "../components/BandsPanel";
import { BookmarksPanel } from "../components/BookmarksPanel";
import { segment } from "../components/controls";
import { OccupancyPanel } from "../components/OccupancyPanel";
import { PresetsPanel } from "../components/PresetsPanel";
import { RecordingsPanel } from "../components/RecordingsPanel";
import { TemplatesPanel } from "../components/TemplatesPanel";
import type { RecordingInfo } from "../lib/types";
import { ToolsPanel } from "../tools/ToolsPanel";
import { useWorkspaceContext } from "./context";
import { FieldPanel } from "./FieldPanel";
import { addNode, newNodeId, nodeIds } from "./graph";
import { libraryTarget } from "./libraryTarget";
import { recordingNodeFor } from "./nodes/recordingNode";
import { useNodePlacement } from "./placement";

const TABS = [
  { id: "templates", label: "Templates" },
  { id: "presets", label: "Presets" },
  { id: "bookmarks", label: "Bookmarks" },
  { id: "bands", label: "Bands" },
  { id: "occupancy", label: "Occupancy" },
  { id: "recordings", label: "Recordings" },
  { id: "tools", label: "Tools" },
  { id: "field", label: "Field" },
] as const;

export function Library({ onOpenTool }: { onOpenTool: (id: string) => void }) {
  const workspace = useWorkspaceContext();
  const placeNode = useNodePlacement();
  const selected =
    workspace.selected === null ? null : (workspace.devices.get(workspace.selected) ?? null);
  const drawn = [...workspace.devices.values()];
  const only = drawn.length === 1 ? (drawn[0] ?? null) : null;
  const active = selected ?? only;
  const target = libraryTarget(workspace.graph, workspace.devices, workspace.selected);

  const openRecording = (recording: RecordingInfo): void => {
    const id = newNodeId("recording", nodeIds(workspace.graph));
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: addNode(
        snapshot.graph,
        recordingNodeFor(recording, id, placeNode(snapshot.graph, "recording")),
      ),
    }));
    workspace.select(id);
    workspace.apply();
  };

  return (
    <Tabs.Root defaultValue="templates" className="flex flex-col overflow-hidden rounded-md">
      <Tabs.List
        className="flex shrink-0 items-center gap-0.5 border-b border-line bg-panel-2 px-2 py-1.5"
        aria-label="Library section"
      >
        {TABS.map((entry) => (
          <Tabs.Tab key={entry.id} value={entry.id} className={(state) => segment(state.active)}>
            {entry.label}
          </Tabs.Tab>
        ))}
      </Tabs.List>
      <Tabs.Panel value="templates" className="max-h-[28rem] overflow-y-auto">
        <TemplatesPanel active={active} onApplied={() => workspace.apply()} />
      </Tabs.Panel>
      <Tabs.Panel value="presets" className="max-h-[28rem] overflow-y-auto">
        <PresetsPanel />
      </Tabs.Panel>
      <Tabs.Panel value="bookmarks" className="max-h-[28rem] overflow-y-auto">
        <BookmarksPanel target={target} />
      </Tabs.Panel>
      <Tabs.Panel value="bands" className="max-h-[28rem] overflow-y-auto">
        <BandsPanel target={target} />
      </Tabs.Panel>
      <Tabs.Panel value="occupancy" className="max-h-[28rem] overflow-y-auto">
        <OccupancyPanel active={active} />
      </Tabs.Panel>
      <Tabs.Panel value="recordings" className="max-h-[28rem] overflow-y-auto">
        <RecordingsPanel onOpen={openRecording} />
      </Tabs.Panel>
      <Tabs.Panel value="tools" className="max-h-[28rem] overflow-y-auto">
        <ToolsPanel onOpen={onOpenTool} />
      </Tabs.Panel>
      <Tabs.Panel value="field" className="max-h-[28rem] overflow-y-auto">
        <FieldPanel />
      </Tabs.Panel>
    </Tabs.Root>
  );
}
