import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { SettingRow, Settings } from "../../components/Settings";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";

export function SpectrumMonitorFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  if (node.kind !== "spectrum_monitor") return null;
  return (
    <NodeShell node={node} title="Spectrum monitor" category="tool">
      <FaceBody>
        <Settings className="p-2">
          <SettingRow
            label="Min confidence (%)"
            title="Ignore signals below this identification confidence; 0 accepts all detections"
          >
            <NumberField
              label="Minimum confidence (%)"
              value={Math.round((node.data.min_confidence ?? 0.7) * 100)}
              min={0}
              max={100}
              step={5}
              onCommit={(value) =>
                workspace.edit((snapshot) => ({
                  ...snapshot,
                  graph: patchNode(snapshot.graph, node.id, (current) =>
                    current.kind === "spectrum_monitor"
                      ? { ...current, data: { ...current.data, min_confidence: value / 100 } }
                      : current,
                  ),
                }))
              }
            />
          </SettingRow>
          <SettingRow
            label="Record audio"
            title="Attach temporary audio clips to transmission events"
          >
            <Checkbox
              label="Record audio"
              checked={node.data.record_audio ?? true}
              onChange={(record_audio) =>
                workspace.edit((snapshot) => ({
                  ...snapshot,
                  graph: patchNode(snapshot.graph, node.id, (current) =>
                    current.kind === "spectrum_monitor"
                      ? { ...current, data: { ...current.data, record_audio } }
                      : current,
                  ),
                }))
              }
            />
          </SettingRow>
        </Settings>
      </FaceBody>
    </NodeShell>
  );
}
