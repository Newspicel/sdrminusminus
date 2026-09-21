import { Checkbox } from "../../components/Checkbox";
import { formatHz } from "../../components/format";
import { SettingRow, Settings } from "../../components/Settings";
import { useDecodedKind } from "../../lib/decoded";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode, portStream } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";
import { monitorTransmissions } from "./spectrumMonitor";

export function SpectrumMonitorFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const records = useDecodedKind("transmission");
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
  const rows = monitorTransmissions(records, node.id);
  const problem = rows.find((row) => row.state === "problem");
  const active = rows.filter((row) => row.state === "started" || row.state === "continued");
  const state =
    edge === undefined
      ? "Connect IQ"
      : device?.status === "running"
        ? "Monitoring"
        : "Waiting for IQ";
  return (
    <NodeShell node={node} title="Spectrum monitor" category="tool" subtitle={state}>
      <FaceBody>
        <Settings className="border-b border-line p-2">
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
        <div className="flex justify-between gap-2 p-2 text-xs tabular-nums">
          <span>{device?.status === "running" ? active.length : 0} active</span>
          <span>{rows.filter((row) => row.state !== "problem").length} recent</span>
        </div>
        {problem?.error && (
          <div
            role="status"
            className="truncate px-2 pb-2 text-xs text-warning"
            title={problem.error}
          >
            {problem.error}
          </div>
        )}
        <ul className="text-xs tabular-nums">
          {rows
            .filter((row) => row.state !== "problem")
            .slice(0, 5)
            .map((row) => (
              <li
                key={row.id}
                className="flex items-center justify-between gap-2 border-t border-line px-2 py-1"
                title={row.error ?? row.state}
              >
                <span>{formatHz(Math.round(row.signal.frequency_hz))}</span>
                <span className="truncate">
                  {row.decoder?.toUpperCase() ?? row.signal.modulation}
                </span>
                <span className="text-ink-dim">{row.state}</span>
              </li>
            ))}
        </ul>
      </FaceBody>
    </NodeShell>
  );
}
