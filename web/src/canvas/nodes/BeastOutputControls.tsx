import { useEffect, useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER } from "../../components/controls";
import type { EventOutputTarget, ServerEvent } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { FaceFooter } from "./NodeShell";

type BeastTarget = Extract<EventOutputTarget, { service: "beast" }>;
type BeastStatus = Extract<ServerEvent, { type: "BeastExportStatus" }>["data"];

export function BeastOutputControls({
  node,
  target,
  connected,
  onEdit,
}: {
  node: string;
  target: BeastTarget;
  connected: boolean;
  onEdit: (target: BeastTarget) => void;
}) {
  const { socket } = useWorkspaceContext();
  const [status, setStatus] = useState<BeastStatus | null>(null);
  useEffect(() => {
    const offEvent = socket.on("event", (event: ServerEvent) => {
      if (event.type === "BeastExportStatus" && event.data.node === node) setStatus(event.data);
    });
    const offStatus = socket.on("status", () => setStatus(null));
    return () => {
      offEvent();
      offStatus();
    };
  }, [socket, node]);
  const current = connected && target.enabled && status?.address === target.address ? status : null;
  return (
    <>
      <div className="p-2 font-mono text-xs tabular-nums">
        {current?.error ? (
          <p role="alert" className="text-danger">
            {current.error}
          </p>
        ) : null}
        {current?.listening
          ? `${current.clients} clients · ${current.frames.toLocaleString()} frames`
          : current?.error
            ? "Server failed"
            : connected
              ? target.enabled
                ? "Opening server"
                : "Server closed"
              : "Wire ADS-B events in"}
      </div>
      <FaceFooter>
        <Button
          type="button"
          className={target.enabled ? BTN_DANGER : BTN}
          disabled={!target.enabled && (!connected || target.address.trim() === "")}
          onClick={() => {
            setStatus(null);
            onEdit({ ...target, enabled: !target.enabled });
          }}
        >
          {target.enabled ? "Close server" : "Open server"}
        </Button>
      </FaceFooter>
    </>
  );
}
