import { useQuery } from "@tanstack/react-query";
import { CHIP } from "../../components/controls";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { callsQuery } from "../../lib/api";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { DMR_TRUNK_PROTOCOLS, dmrTrunkGuidance } from "./dmrTrunk";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { CallRow } from "./SinkFaces";

const RETENTION_OPTIONS = [
  { value: 0, label: "Off" },
  { value: 60, label: "1 minute" },
  { value: 300, label: "5 minutes" },
  { value: 900, label: "15 minutes" },
  { value: 3_600, label: "1 hour" },
  { value: 21_600, label: "6 hours" },
] as const;

export function DmrTrunkFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const result = useQuery(callsQuery());
  if (node.kind !== "dmr_trunk") {
    return null;
  }
  const calls = (result.data?.calls ?? []).filter((call) => call.node === node.id).slice(0, 20);
  const status = workspace.trunks.find((system) => system.node === node.id);
  const sources = (workspace.graph.edges ?? [])
    .filter((edge) => edge.to.node === node.id && edge.to.port === "events")
    .map((edge) => edge.from.node);
  const protocol = node.data.protocol ?? "auto";
  const edit = (next: Partial<typeof node.data>) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "dmr_trunk" ? { ...current, data: { ...current.data, ...next } } : current,
      ),
    }));
  };
  return (
    <NodeShell
      node={node}
      title="DMR trunk system"
      category="feature"
      subtitle={
        sources.length > 0
          ? `${sources.length} carrier${sources.length === 1 ? "" : "s"} · ${status?.followers.length ?? 0} following · ${calls.length} calls`
          : undefined
      }
      live={sources.length > 0}
    >
      <FaceBody>
        <Settings className="border-b border-line p-2">
          <SettingRow label="Protocol">
            <Select
              label="Protocol"
              value={protocol}
              options={DMR_TRUNK_PROTOCOLS}
              onChange={(next) => edit({ protocol: next })}
            />
          </SettingRow>
          <SettingRow label="Keep calls">
            <Select
              label="Keep calls"
              value={node.data.retention_seconds ?? 300}
              options={RETENTION_OPTIONS}
              onChange={(next) => edit({ retention_seconds: next })}
            />
          </SettingRow>
        </Settings>
        {sources.length === 0 ? (
          <FaceEmpty>Wire DMR decoder events into the events input.</FaceEmpty>
        ) : (
          <>
            <p className="border-b border-line p-2 text-xs text-ink-dim">
              {dmrTrunkGuidance(protocol, status?.detected ?? null)} Runs on the server while this
              page is closed.
            </p>
            {status !== undefined && status.followers.length > 0 && (
              <ul className="flex flex-wrap gap-1 border-b border-line p-2">
                {status.followers.map((follower) => (
                  <li key={`${follower.freq_hz}-${follower.slot}`} className={CHIP}>
                    {(follower.freq_hz / 1e6).toFixed(4)} MHz TS {follower.slot}
                    {follower.logical_channel == null ? "" : ` · LCN ${follower.logical_channel}`}
                  </li>
                ))}
              </ul>
            )}
            {status?.problems.map((problem) => (
              <p
                key={`${problem.freq_hz}-${problem.slot}`}
                role="alert"
                className="border-b border-line p-2 text-xs text-warning"
              >
                Cannot follow {(problem.freq_hz / 1e6).toFixed(4)} MHz TS {problem.slot}:{" "}
                {problem.reason}
              </p>
            ))}
            {result.isError ? (
              <p role="alert" className="p-3 text-xs text-danger">
                {result.error.message}
              </p>
            ) : calls.length === 0 ? (
              <FaceEmpty>Waiting for a completed call.</FaceEmpty>
            ) : (
              calls.map((call) => <CallRow key={call.id} call={call} />)
            )}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}
