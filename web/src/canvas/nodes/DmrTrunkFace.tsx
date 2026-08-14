import { useQuery } from "@tanstack/react-query";
import { CHIP, LABEL } from "../../components/controls";
import { Select } from "../../components/Select";
import { callsQuery } from "../../lib/api";
import type { DmrTrunkProtocol, DvTrunkProtocol, PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { CallRow } from "./SinkFaces";

export const DMR_TRUNK_PROTOCOLS: readonly { value: DmrTrunkProtocol; label: string }[] = [
  { value: "auto", label: "Auto-detect" },
  { value: "capacity_plus", label: "Capacity Plus" },
  { value: "hytera_xpt", label: "Hytera XPT" },
  { value: "tier_three", label: "Tier III / Capacity Max" },
];

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
        <div className="flex flex-wrap items-end gap-3 border-b border-line p-2">
          <label className="flex flex-col gap-1">
            <span className={LABEL}>Protocol</span>
            <Select
              label="Protocol"
              className="w-48"
              value={protocol}
              options={DMR_TRUNK_PROTOCOLS}
              onChange={(next) => edit({ protocol: next })}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className={LABEL}>Keep calls</span>
            <Select
              label="Keep calls"
              className="w-28"
              value={node.data.retention_seconds ?? 300}
              options={RETENTION_OPTIONS}
              onChange={(next) => edit({ retention_seconds: next })}
            />
          </label>
        </div>
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

export function dmrTrunkGuidance(
  protocol: DmrTrunkProtocol,
  detected: DvTrunkProtocol | null = null,
): string {
  if (protocol === "auto" && detected !== null) {
    switch (detected) {
      case "capacity_plus":
        return "Detected Capacity Plus signalling; both timeslots of every wired carrier are being followed.";
      case "hytera_xpt":
        return "Detected Hytera XPT signalling; both timeslots of every wired carrier are being followed.";
      case "tier_three":
        return "Detected Tier III signalling; voice grants create traffic receivers automatically.";
    }
  }
  switch (protocol) {
    case "capacity_plus":
      return "Add one DMR decoder for every known repeater output frequency. Both timeslots are isolated automatically.";
    case "hytera_xpt":
      return "Add one DMR decoder for every Hytera XPT repeater output frequency. Both timeslots are isolated automatically.";
    case "tier_three":
      return "Add the DMR control-channel decoder. Standard channel definitions and voice grants create traffic receivers automatically.";
    case "auto":
      return "The system detects Capacity Plus, Hytera XPT, or Tier III signalling from the connected DMR carriers.";
  }
}
