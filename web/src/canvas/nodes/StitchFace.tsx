import type { Options } from "../../components/controls";
import { formatMhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import type { PatchNode, StitchMode, StitchParams } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { FaceBody, NodeShell } from "./NodeShell";
import { DEFAULT_STITCH_PARAMS, STITCH_MODE_NOTE } from "./stitch";

const MODES: Options<StitchMode> = [
  { value: "auto", label: "Auto" },
  { value: "manual", label: "Manual" },
];

export function StitchFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const lane = deviceSetOf(workspace, node.id)?.extra_lane ?? null;
  if (node.kind !== "stitch") {
    return null;
  }
  const settings = node.data.settings ?? DEFAULT_STITCH_PARAMS;
  const update = (next: Partial<StitchParams>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "stitch"
          ? {
              ...current,
              data: { settings: { ...(current.data.settings ?? DEFAULT_STITCH_PARAMS), ...next } },
            }
          : current,
      ),
    }));
  };
  return (
    <NodeShell node={node} title="Stitch" category="tool">
      <FaceBody>
        {lane !== null && (
          <Readout separated={false}>
            <ReadoutRow label="Center">{formatMhz(lane.center_hz)}</ReadoutRow>
            <ReadoutRow label="Rate">{formatMhz(lane.sample_rate)}</ReadoutRow>
          </Readout>
        )}
        <Settings className="border-t border-line p-2">
          <SettingRow label="Mode" title={STITCH_MODE_NOTE[settings.mode]}>
            <Select
              label="How the lanes are placed"
              value={settings.mode}
              onChange={(mode) => update({ mode })}
              options={MODES}
            />
          </SettingRow>
          <SettingRow label="Lanes">
            <NumberField
              label="How many lanes are wired in"
              value={settings.lanes}
              min={2}
              max={16}
              step={1}
              onCommit={(lanes) => update({ lanes })}
            />
          </SettingRow>
        </Settings>
      </FaceBody>
    </NodeShell>
  );
}
