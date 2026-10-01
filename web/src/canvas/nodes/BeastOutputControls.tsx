import { useEffect, useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER } from "../../components/controls";
import { FaceFault } from "../../components/face/Fault";
import { FaceStats, Stat } from "../../components/face/Stats";
import { formatCount } from "../../components/format";
import type { EventOutputTarget, ServerEvent } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { FaceEmpty, FaceFooter } from "./NodeShell";

type BeastTarget = Extract<EventOutputTarget, { service: "beast" }>;
type BeastStatus = Extract<ServerEvent, { type: "BeastExportStatus" }>["data"];

export function beastState(
  current: BeastStatus | null,
  connected: boolean,
  enabled: boolean,
): string | undefined {
  if (current?.error) {
    return undefined;
  }
  if (current?.listening) {
    return "Listening";
  }
  if (!connected) {
    return "Wire ADS-B events in";
  }
  return enabled ? "Opening server" : "Server closed";
}

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
      <FaceEmpty hint={beastState(current, connected, target.enabled === true)} />
      {current?.error ? <FaceFault message="Server failed" detail={current.error} /> : null}
      <FaceFooter>
        {current?.listening && (
          <FaceStats>
            <Stat label="Clients" title="Feeders connected to this server">
              {current.clients}
            </Stat>
            <Stat label="Frames" title="Beast frames sent">
              {formatCount(current.frames)}
            </Stat>
          </FaceStats>
        )}
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
