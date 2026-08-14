import { useQuery } from "@tanstack/react-query";
import { FIELD, LABEL } from "../../components/controls";
import { callsQuery } from "../../lib/api";
import type { DmrTrunkProtocol, PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { CallRow } from "./SinkFaces";

const PROTOCOLS: readonly { value: DmrTrunkProtocol; label: string }[] = [
  { value: "auto", label: "Auto-detect" },
  { value: "capacity_plus", label: "Capacity Plus" },
  { value: "tier_three", label: "Tier III / Capacity Max" },
];

export function DmrTrunkFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const result = useQuery(callsQuery());
  if (node.kind !== "dmr_trunk") {
    return null;
  }
  const calls = (result.data?.calls ?? []).filter((call) => call.node === node.id).slice(0, 20);
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
          ? `${sources.length} carrier${sources.length === 1 ? "" : "s"} · ${calls.length} calls`
          : undefined
      }
      live={sources.length > 0}
    >
      <FaceBody>
        <div className="flex flex-wrap items-end gap-3 border-b border-line p-2">
          <label className="flex flex-col gap-1">
            <span className={LABEL}>Protocol</span>
            <select
              className={`${FIELD} w-48`}
              value={protocol}
              onChange={(event) => edit({ protocol: event.target.value as DmrTrunkProtocol })}
            >
              {PROTOCOLS.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <label className="flex flex-col gap-1">
            <span className={LABEL}>Keep calls</span>
            <select
              className={`${FIELD} w-28`}
              value={node.data.retention_seconds ?? 300}
              onChange={(event) => edit({ retention_seconds: Number(event.target.value) })}
            >
              <option value={0}>Off</option>
              <option value={60}>1 minute</option>
              <option value={300}>5 minutes</option>
              <option value={900}>15 minutes</option>
              <option value={3600}>1 hour</option>
              <option value={21600}>6 hours</option>
            </select>
          </label>
        </div>
        {sources.length === 0 ? (
          <FaceEmpty>Wire DMR decoder events into the events input.</FaceEmpty>
        ) : (
          <>
            <p className="border-b border-line p-2 text-xs text-ink-dim">
              {guidance(protocol)} Runs on the server while this page is closed.
            </p>
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

function guidance(protocol: DmrTrunkProtocol): string {
  switch (protocol) {
    case "capacity_plus":
      return "Add one DMR decoder for every known repeater output frequency. Both timeslots are isolated automatically.";
    case "tier_three":
      return "Add the DMR control-channel decoder. Standard channel definitions and voice grants create traffic receivers automatically.";
    case "auto":
      return "The system detects Capacity Plus or Tier III signalling from the connected DMR carriers.";
  }
}
