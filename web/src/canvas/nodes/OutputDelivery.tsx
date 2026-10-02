import { useEffect, useState } from "react";
import { FaceFault } from "../../components/face/Fault";
import { FaceStats, Stat } from "../../components/face/Stats";
import { formatCount } from "../../components/format";
import type { ServerEvent } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { FaceFooter } from "./NodeShell";

export type DeliveryStatus = Extract<ServerEvent, { type: "EventOutputStatus" }>["data"];

export function OutputDelivery({ node }: { node: string }) {
  const { socket } = useWorkspaceContext();
  const [status, setStatus] = useState<DeliveryStatus | null>(null);
  useEffect(() => {
    const offEvent = socket.on("event", (event: ServerEvent) => {
      if (event.type === "EventOutputStatus" && event.data.node === node) setStatus(event.data);
    });
    const offStatus = socket.on("status", () => setStatus(null));
    return () => {
      offEvent();
      offStatus();
    };
  }, [socket, node]);
  return <DeliveryReadout status={status} />;
}

export function DeliveryReadout({ status }: { status: DeliveryStatus | null }) {
  if (status === null) {
    return null;
  }
  return (
    <>
      {status.error ? <FaceFault message="Delivery failed" detail={status.error} /> : null}
      {status.delivered + status.failed > 0 && (
        <FaceFooter>
          <FaceStats>
            <Stat label="Sent" title="Events delivered">
              {formatCount(status.delivered)}
            </Stat>
            <Stat label="Failed" title="Events not delivered">
              {formatCount(status.failed)}
            </Stat>
          </FaceStats>
        </FaceFooter>
      )}
    </>
  );
}
