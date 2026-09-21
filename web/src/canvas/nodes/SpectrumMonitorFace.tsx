import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { SettingRow, Settings } from "../../components/Settings";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode, portStream } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";

export function SpectrumMonitorFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  if (node.kind !== "spectrum_monitor") return null;
  const edge = workspace.graph.edges?.find(
    (wire) => wire.to.node === node.id && wire.to.port === "iq",
  );
  const source = workspace.graph.nodes.find((candidate) => candidate.id === edge?.from.node);
  const upstream =
    source?.kind === "df" || source?.kind === "combiner"
      ? workspace.graph.edges?.find(
          (wire) => wire.to.node === source.id && portStream("iq", wire.to.port) !== null,
        )?.from.node
      : edge?.from.node;
  const device = upstream === undefined ? undefined : workspace.devices.get(upstream);
  const state =
    edge === undefined
      ? "Connect IQ"
      : device?.status === "running"
        ? "Monitoring"
        : "Waiting for IQ";
  return (
    <NodeShell node={node} title="Spectrum monitor" category="tool" subtitle={state}>
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
