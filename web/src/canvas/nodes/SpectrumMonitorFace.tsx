import { Chips, NumberChip, ToggleChip } from "../../components/face/Chips";
import type { PatchNode, SpectrumMonitorNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";
import { ProtocolChip } from "./ProtocolPicker";
import { protocolGroups } from "./protocols";

const DEFAULT_CONFIDENCE = 0.7;

export function SpectrumMonitorFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  if (node.kind !== "spectrum_monitor") return null;
  const edit = (next: Partial<SpectrumMonitorNode>) =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "spectrum_monitor"
          ? { ...current, data: { ...current.data, ...next } }
          : current,
      ),
    }));
  const confidence = Math.round((node.data.min_confidence ?? DEFAULT_CONFIDENCE) * 100);
  return (
    <NodeShell node={node} title="Spectrum monitor" category="tool">
      <FaceBody>
        <Chips className="p-2">
          <ProtocolChip
            groups={protocolGroups(workspace.context.channelTypes)}
            choice={{
              disabled: node.data.disabled_protocols ?? [],
              unidentified: node.data.report_unidentified ?? true,
            }}
            onChange={(choice) =>
              edit({
                disabled_protocols: [...choice.disabled],
                report_unidentified: choice.unidentified,
              })
            }
          />
          <NumberChip
            label="Min"
            title="Minimum confidence. Signals identified below it are ignored; 0 accepts all"
            value={confidence}
            unit="%"
            min={0}
            max={100}
            step={5}
            onCommit={(value) => edit({ min_confidence: value / 100 })}
          />
          <ToggleChip
            label="Clips"
            title="Attach temporary audio clips to transmission events"
            on={node.data.record_audio ?? true}
            onChange={(record_audio) => edit({ record_audio })}
          />
        </Chips>
      </FaceBody>
    </NodeShell>
  );
}
