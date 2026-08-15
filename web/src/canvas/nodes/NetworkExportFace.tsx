import { useMutation } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN, BTN_DANGER, FIELD } from "../../components/controls";
import {
  deriveNetworkExportControl,
  networkExportControlsLocked,
  networkExportMutationOptions,
} from "../../components/networkExport";
import { formatBytes } from "../../components/recordings";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import type { PatchNode, PatchNodeOf } from "../../lib/types";
import { iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { FaceBody, FaceEmpty, FaceFooter, NodeShell } from "./NodeShell";

const TRANSPORTS = [
  { value: "udp", label: "UDP datagrams" },
  { value: "tcp", label: "TCP stream" },
] as const;

const FORMATS = [
  { value: "cf32_le", label: "Complex float 32 LE" },
  { value: "ci16_le", label: "Complex int 16 LE" },
  { value: "cu8", label: "Complex unsigned 8" },
] as const;

export function NetworkExportFace({ node }: { node: PatchNode }) {
  if (node.kind !== "network_export") {
    return null;
  }
  return <NetworkExportNodeFace node={node} />;
}

function NetworkExportNodeFace({ node }: { node: PatchNodeOf<"network_export"> }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const source = iqSourceOf(workspace.graph, node.id);
  const [address, setAddress] = useState(node.data.address);

  useEffect(() => setAddress(node.data.address), [node.data.address]);
  const control = deriveNetworkExportControl(set, node.id);
  const settings = {
    transport: node.data.transport,
    format: node.data.format,
    address: address.trim() || node.data.address,
  };
  const edit = (next: Partial<typeof settings>) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "network_export"
          ? { ...current, data: { ...current.data, ...next } }
          : current,
      ),
    }));
  };
  const exportIq = useMutation(
    networkExportMutationOptions(
      set === null || source === null ? null : { deviceSet: set.id, stream: source.stream },
      node.id,
      settings,
    ),
  );
  const locked = networkExportControlsLocked(control, exportIq.isPending);

  return (
    <NodeShell
      node={node}
      title="Network IQ"
      category="sink"
      subtitle={control.kind === "active" ? node.data.address : undefined}
      live={control.kind === "active"}
    >
      <FaceBody>
        <Settings className="border-b border-line p-2">
          <SettingRow label="Transport">
            <Select
              label="Transport"
              value={node.data.transport}
              options={TRANSPORTS}
              disabled={locked}
              onChange={(transport) => edit({ transport })}
            />
          </SettingRow>
          <SettingRow label="Samples">
            <Select
              label="Sample format"
              value={node.data.format}
              options={FORMATS}
              disabled={locked}
              onChange={(format) => edit({ format })}
            />
          </SettingRow>
          <SettingRow label="Destination">
            <Input
              className={FIELD}
              aria-label="Network IQ destination"
              value={address}
              disabled={locked}
              onChange={(event) => setAddress(event.target.value)}
              onBlur={() => {
                const next = address.trim();
                if (next !== "" && next !== node.data.address) edit({ address: next });
                else setAddress(node.data.address);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
              }}
            />
          </SettingRow>
        </Settings>
        {source === null || set === null ? (
          <FaceEmpty>Wire a device's IQ out into this sink.</FaceEmpty>
        ) : control.kind === "active" ? (
          <div className="grid grid-cols-2 gap-x-3 gap-y-1 p-2 font-mono text-xs tabular-nums">
            <span className="text-ink-dim">Rate</span>
            <span>{control.status.sample_rate.toLocaleString()} S/s</span>
            <span className="text-ink-dim">Center</span>
            <span>{(control.status.center_hz / 1e6).toFixed(6)} MHz</span>
            <span className="text-ink-dim">Sent</span>
            <span>{formatBytes(control.status.bytes)}</span>
            <span className="text-ink-dim">
              {control.status.settings.transport === "udp" ? "Datagrams" : "Writes"}
            </span>
            <span>{control.status.packets.toLocaleString()}</span>
            <span className="text-ink-dim">Capture loss</span>
            <span>{control.status.overruns.toLocaleString()} samples</span>
            {control.status.error != null && (
              <p role="alert" className="col-span-2 text-danger">
                {control.status.error}
              </p>
            )}
          </div>
        ) : (
          <FaceEmpty>
            {control.kind === "busy"
              ? "Another network sink is already using this radio."
              : control.kind === "ready"
                ? "Raw interleaved I/Q. Set the same rate and format in the receiving tool."
                : "The radio has to be running before it can export."}
          </FaceEmpty>
        )}
      </FaceBody>
      <FaceFooter>
        {control.kind === "active" ? (
          <Button
            type="button"
            className={BTN_DANGER}
            disabled={exportIq.isPending}
            onClick={() => exportIq.mutate("stop")}
          >
            Stop
          </Button>
        ) : (
          <Button
            type="button"
            className={BTN}
            disabled={control.kind !== "ready" || exportIq.isPending}
            onClick={() => exportIq.mutate("start")}
          >
            Start export
          </Button>
        )}
      </FaceFooter>
    </NodeShell>
  );
}
