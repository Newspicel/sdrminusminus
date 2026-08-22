import { isPinned, patchNode, pin, unpin } from "./canvas/graph";
import { deviceDialId } from "./canvas/nodes/deviceNode";
import { useHotkeys } from "./canvas/useHotkeys";
import type { WorkspaceStore } from "./canvas/useWorkspace";
import type { View } from "./canvas/WorkspaceBar";
import { TUNE_STEPS_HZ, tuningRange } from "./components/dial";
import type { ChannelInfo, DeviceSet, PatchGraph, PatchNode } from "./lib/types";
import type { useChannelPatch } from "./lib/useChannelPatch";
import type { useDevicePatch } from "./lib/useDevicePatch";

const MODE_RING = ["nfm", "wfm", "am", "ssb"] as const;

export interface AppHotkeys {
  selected: string | null;
  setSelected: (id: string | null) => void;
  selectedSet: DeviceSet | null;
  selectedChannel: ChannelInfo | null;
  selectedNode: PatchNode | null;
  selectedDevice: string | null;
  channelNodes: readonly PatchNode[];
  graph: PatchGraph;
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
      if (b.selectedSet === null) {
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
        document.getElementById(deviceDialId(b.selectedDevice))?.focus();
      }
    },
    cycleMode: (direction) => {
      if (b.selectedSet === null || b.selectedChannel === null) {
        return;
      }
      const at = MODE_RING.indexOf(
        b.selectedChannel.settings.params.type as (typeof MODE_RING)[number],
      );
      const next = MODE_RING[(Math.max(0, at) + direction + MODE_RING.length) % MODE_RING.length];
      const target = b.selected;
      if (next === undefined || target === null) {
        return;
      }
      b.applyEdit(b.selectedSet.id, b.selectedChannel.id, { params: { type: next, settings: {} } });
      b.workspace.save((current) => ({
        ...current,
        graph: patchNode(current.graph, target, (node) =>
          node.kind === "channel"
            ? { ...node, kind: "channel" as const, data: { channel_type: next } }
            : node,
        ),
      }));
    },
    adjustSquelch: (deltaDb) => {
      if (b.selectedSet === null || b.selectedChannel === null) {
        return;
      }
      b.applyEdit(b.selectedSet.id, b.selectedChannel.id, (current) => ({
        squelch_db: Math.min(0, Math.max(-120, (current.squelch_db ?? -60) + deltaDb)),
      }));
    },
    toggleSquelch: () => {
      if (b.selectedSet === null || b.selectedChannel === null) {
        return;
      }
      b.applyEdit(b.selectedSet.id, b.selectedChannel.id, (current) => ({
        squelch_db: current.squelch_db == null ? -60 : null,
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
