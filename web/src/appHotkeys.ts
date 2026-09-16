import { type GraphContext, isPinned, pin, tuningLocked, unpin } from "./canvas/graph";
import { nextAnalogMode, swapDecoder } from "./canvas/nodes/decoderSwap";
import { useHotkeys } from "./canvas/useHotkeys";
import type { WorkspaceStore } from "./canvas/useWorkspace";
import type { View } from "./canvas/WorkspaceBar";
import { DEFAULT_SQUELCH_DB, nudgedSquelch, SQUELCH_OFF } from "./components/channelSettings";
import { TUNE_STEPS_HZ, tuningRange } from "./components/dial";
import { dialId } from "./components/FrequencyDial";
import type { ChannelInfo, DeviceSet, PatchGraph, PatchNode } from "./lib/types";
import type { useChannelPatch } from "./lib/useChannelPatch";
import type { useDevicePatch } from "./lib/useDevicePatch";

export interface AppHotkeys {
  selected: string | null;
  setSelected: (id: string | null) => void;
  selectedSet: DeviceSet | null;
  selectedChannel: ChannelInfo | null;
  selectedNode: PatchNode | null;
  selectedDevice: string | null;
  channelNodes: readonly PatchNode[];
  graph: PatchGraph;
  context: GraphContext;
  stepHz: number;
  setStepHz: (hz: number) => void;
  workspace: WorkspaceStore;
  applyPatch: ReturnType<typeof useDevicePatch>["applyPatch"];
  cachedSettings: ReturnType<typeof useDevicePatch>["cachedSettings"];
  applyEdit: ReturnType<typeof useChannelPatch>["applyEdit"];
  setView: (update: (current: View) => View) => void;
  setExpanded: (update: (current: string | null) => string | null) => void;
  setShowShortcuts: (show: boolean) => void;
}

export function useAppHotkeys(b: AppHotkeys) {
  useHotkeys({
    tune: (steps) => {
      if (
        b.selectedSet === null ||
        b.selectedDevice === null ||
        tuningLocked(b.graph, b.selectedDevice)
      ) {
        return;
      }
      const range = tuningRange(b.selectedSet.capabilities);
      const current = b.cachedSettings(b.selectedSet.id)?.center_hz ?? 0;
      const wanted = current + steps * b.stepHz;
      b.applyPatch(b.selectedSet.id, {
        center_hz: Math.min(range.max, Math.max(range.min, wanted)),
      });
    },
    stepBy: (direction) => {
      const at = TUNE_STEPS_HZ.indexOf(b.stepHz as (typeof TUNE_STEPS_HZ)[number]);
      const next = Math.min(TUNE_STEPS_HZ.length - 1, Math.max(0, at + direction));
      b.setStepHz(TUNE_STEPS_HZ[next] ?? b.stepHz);
    },
    focusDial: () => {
      if (b.selectedDevice !== null) {
        document.getElementById(dialId(b.selectedDevice))?.focus();
      }
    },
    cycleMode: (direction) => {
      const node = b.selectedNode;
      if (node === null || node.kind !== "channel") {
        return;
      }
      const wanted = nextAnalogMode(node.data.channel_type, direction);
      const descriptor = b.context.channelTypes.find((type) => type.type_id === wanted);
      if (descriptor === undefined) {
        return;
      }
      swapDecoder({
        context: b.context,
        node,
        descriptor,
        live:
          b.selectedSet === null || b.selectedChannel === null
            ? null
            : { deviceSet: b.selectedSet.id, channel: b.selectedChannel },
        saved: b.workspace.savedChannels.get(node.id) ?? null,
        applyEdit: b.applyEdit,
        saveChannel: b.workspace.saveChannel,
        edit: b.workspace.save,
      });
    },
    adjustSquelch: (deltaDb) => {
      if (b.selectedSet === null || b.selectedChannel === null) {
        return;
      }
      b.applyEdit(b.selectedSet.id, b.selectedChannel.id, (current) => ({
        squelch: nudgedSquelch(current.squelch, deltaDb),
      }));
    },
    toggleSquelch: () => {
      if (b.selectedSet === null || b.selectedChannel === null) {
        return;
      }
      b.applyEdit(b.selectedSet.id, b.selectedChannel.id, (current) => ({
        squelch:
          current.squelch?.mode === "off" || current.squelch === undefined
            ? { mode: "manual", level_db: DEFAULT_SQUELCH_DB }
            : SQUELCH_OFF,
      }));
    },
    selectChannel: (direction) => {
      if (b.channelNodes.length === 0) {
        return;
      }
      const at = b.channelNodes.findIndex((node) => node.id === b.selected);
      const next = b.channelNodes[(at + direction + b.channelNodes.length) % b.channelNodes.length];
      b.setSelected(next?.id ?? null);
    },
    selectNode: (index) => b.setSelected(b.graph.nodes[index]?.id ?? null),
    togglePin: () => {
      const node = b.selectedNode;
      if (node === null) {
        return;
      }
      b.workspace.save((current) => ({
        ...current,
        rack: isPinned(current.rack ?? {}, node.id)
          ? unpin(current.rack ?? {}, node.id)
          : pin(current.rack ?? {}, node.id),
      }));
    },
    toggleView: () => b.setView((current) => (current === "patch" ? "rack" : "patch")),
    toggleFull: () => {
      const node = b.selectedNode;
      b.setExpanded((current) => (current !== null || node === null ? null : node.id));
    },
    undo: b.workspace.undo,
    redo: b.workspace.redo,
    showShortcuts: () => b.setShowShortcuts(true),
  });
}
