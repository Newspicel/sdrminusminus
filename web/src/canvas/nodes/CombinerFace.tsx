import { Button } from "../../components/BaseControls";
import { BTN, type Options } from "../../components/controls";
import { formatSignedKhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { calibrateCoherent } from "../../lib/api";
import { useDfStore } from "../../lib/df";
import type { CombineMode, CombinerParams, PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { DEFAULT_COMBINER_PARAMS, MODE_NOTE } from "./combiner";
import { CAL_VERDICT_TEXT, calVerdict, tierLabel } from "./df";
import { FaceBody, NodeShell } from "./NodeShell";

const MODES: Options<CombineMode> = [
  { value: "diversity", label: "Combine" },
  { value: "cancel", label: "Cancel" },
];

export function CombinerFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useDfStore((store) => store.byNode[node.id]);
  if (node.kind !== "combiner") {
    return null;
  }
  const settings = node.data.settings ?? DEFAULT_COMBINER_PARAMS;
  const update = (next: Partial<CombinerParams>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "combiner"
          ? {
              ...current,
              data: {
                settings: { ...(current.data.settings ?? DEFAULT_COMBINER_PARAMS), ...next },
              },
            }
          : current,
      ),
    }));
  };
  const verdict = calVerdict(state?.cal);
  return (
    <NodeShell
      node={node}
      title="Combiner"
      category="channel"
      subtitle={`${settings.lanes} antennas · ${tierLabel(state?.cal)}`}
      live={verdict === "solved"}
    >
      <FaceBody>
        <div className="flex flex-col gap-2 p-2">
          <Readout>
            <ReadoutRow label="Calibration">{CAL_VERDICT_TEXT[verdict]}</ReadoutRow>
          </Readout>
          <Button
            className={BTN}
            type="button"
            onClick={() => {
              void calibrateCoherent(node.id);
            }}
          >
            Calibrate
          </Button>
        </div>
        <Settings className="border-t border-line p-2">
          <SettingRow label="Mode" title={MODE_NOTE[settings.mode]}>
            <Select
              label="What the antennas are added for"
              value={settings.mode}
              onChange={(mode) => update({ mode })}
              options={MODES}
            />
          </SettingRow>
          <SettingRow label="Antennas">
            <NumberField
              label="How many antennas are wired in"
              value={settings.lanes}
              min={2}
              max={16}
              step={1}
              onCommit={(lanes) => update({ lanes })}
            />
          </SettingRow>
          <SettingRow label="Offset" title={formatSignedKhz(settings.offset_hz)}>
            <NumberField
              label="Signal offset in hertz"
              value={settings.offset_hz}
              step={1_000}
              onCommit={(offset_hz) => update({ offset_hz })}
            />
          </SettingRow>
          <SettingRow label="Bandwidth">
            <NumberField
              label="Signal bandwidth in hertz"
              value={settings.bandwidth_hz}
              min={100}
              max={20_000_000}
              step={1_000}
              onCommit={(bandwidth_hz) => update({ bandwidth_hz })}
            />
          </SettingRow>
          <SettingRow label="Solve every">
            <NumberField
              label="How often the weights are solved again, in milliseconds"
              value={settings.update_ms}
              min={100}
              max={10_000}
              step={100}
              onCommit={(update_ms) => update({ update_ms })}
            />
          </SettingRow>
        </Settings>
      </FaceBody>
    </NodeShell>
  );
}
