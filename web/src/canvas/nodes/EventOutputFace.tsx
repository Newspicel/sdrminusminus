import { Chips, ChoiceChip, NumberChip } from "../../components/face/Chips";
import { TextChip } from "../../components/face/TextChip";
import type { EventOutputTarget, PatchNode, PatchNodeOf } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { BeastOutputControls } from "./BeastOutputControls";
import {
  eventOutputConfigured,
  newOutputTarget,
  OUTPUT_SERVICES,
  WEBHOOK_FORMATS,
} from "./eventOutput";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { OutputDelivery } from "./OutputDelivery";

export function EventOutputFace({ node }: { node: PatchNode }) {
  if (node.kind !== "event_output") {
    return null;
  }
  return <EventOutputNodeFace node={node} />;
}

function EventOutputNodeFace({ node }: { node: PatchNodeOf<"event_output"> }) {
  const workspace = useWorkspaceContext();
  const target = node.data.target;
  const inputs = (workspace.graph.edges ?? []).filter(
    (edge) => edge.to.node === node.id && edge.to.port === "events",
  ).length;
  const configured = eventOutputConfigured(target);
  const editTarget = (next: EventOutputTarget) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "event_output" ? { ...current, data: { target: next } } : current,
      ),
    }));
  };
  return (
    <NodeShell node={node} title="Event output" category="output">
      <FaceBody>
        <Chips className="p-2">
          <ChoiceChip
            label="Service"
            title="Output service"
            value={target.service}
            options={OUTPUT_SERVICES}
            onChange={(service) => editTarget(newOutputTarget(service))}
          />
          <TargetChips target={target} onEdit={editTarget} />
        </Chips>
        {target.service === "beast" ? (
          <BeastOutputControls
            node={node.id}
            target={target}
            connected={inputs > 0}
            onEdit={editTarget}
          />
        ) : (
          <>
            <FaceEmpty hint={emptyHint(inputs, configured, target)} />
            <OutputDelivery node={node.id} />
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function emptyHint(inputs: number, configured: boolean, target: EventOutputTarget) {
  if (inputs === 0) {
    return "Wire events in";
  }
  if (target.service === "recordings") {
    return "One WAV per call in Recordings";
  }
  if (target.service === "tunnel") {
    return configured ? "Received IPv4 and IPv6 datagrams" : "Enter the interface name";
  }
  if (!configured) {
    return "Enter the destination credentials";
  }
  if (target.service === "postgres") {
    return "One row per event";
  }
  if (target.service === "influx") {
    return "One point per event";
  }
  return carriesAudio(target)
    ? "One send per event, with available audio"
    : "One send per event, as one JSON object";
}

function carriesAudio(target: EventOutputTarget) {
  return (
    target.service === "matrix" || (target.service === "webhook" && target.format === "discord")
  );
}

type TargetOf<S extends EventOutputTarget["service"]> = Extract<EventOutputTarget, { service: S }>;

function TargetChips({
  target,
  onEdit,
}: {
  target: EventOutputTarget;
  onEdit: (next: EventOutputTarget) => void;
}) {
  switch (target.service) {
    case "recordings":
      return null;
    case "beast":
      return (
        <TextChip
          label="Listen"
          name="Beast listen address"
          title="Beast binary TCP server. Wire ADS-B events in, then connect your feeder to this address."
          value={target.address}
          onCommit={(address) => onEdit({ ...target, address, enabled: false })}
        />
      );
    case "tunnel":
      return <TunnelChips target={target} onEdit={onEdit} />;
    case "webhook":
      return <WebhookChips target={target} onEdit={onEdit} />;
    case "matrix":
      return <MatrixChips target={target} onEdit={onEdit} />;
    case "postgres":
      return <PostgresChips target={target} onEdit={onEdit} />;
    case "influx":
      return <InfluxChips target={target} onEdit={onEdit} />;
    case "mqtt":
      return <MqttChips target={target} onEdit={onEdit} />;
  }
}

function TunnelChips({
  target,
  onEdit,
}: {
  target: TargetOf<"tunnel">;
  onEdit: (next: EventOutputTarget) => void;
}) {
  return (
    <>
      <TextChip
        label="Interface"
        name="Network interface name"
        title="TUN interface name; macOS uses utun followed by a number. Creating an interface requires system networking privileges."
        value={target.interface}
        onCommit={(name) => onEdit({ ...target, interface: name })}
      />
      <TextChip
        label="IPv4"
        name="Interface IPv4 address"
        title="Local IPv4 address of the interface"
        value={target.address}
        onCommit={(address) => onEdit({ ...target, address })}
      />
      <NumberChip
        label="Prefix"
        title="Interface IPv4 prefix length"
        value={target.prefix}
        shown={`/${target.prefix}`}
        min={0}
        max={32}
        step={1}
        onCommit={(prefix) => onEdit({ ...target, prefix })}
      />
    </>
  );
}

function WebhookChips({
  target,
  onEdit,
}: {
  target: TargetOf<"webhook">;
  onEdit: (next: EventOutputTarget) => void;
}) {
  return (
    <>
      <TextChip
        label="Endpoint"
        name="Webhook URL"
        title="Webhook URL"
        value={target.url}
        secret
        onCommit={(url) => onEdit({ ...target, url })}
      />
      <ChoiceChip
        label="Format"
        title="Webhook payload format"
        value={target.format ?? "json"}
        options={WEBHOOK_FORMATS}
        onChange={(format) => onEdit({ ...target, format })}
      />
    </>
  );
}

function MatrixChips({
  target,
  onEdit,
}: {
  target: TargetOf<"matrix">;
  onEdit: (next: EventOutputTarget) => void;
}) {
  return (
    <>
      <TextChip
        label="Homeserver"
        title="Matrix homeserver URL"
        value={target.homeserver_url}
        onCommit={(homeserver_url) => onEdit({ ...target, homeserver_url })}
      />
      <TextChip
        label="Room"
        title="Matrix room ID"
        value={target.room_id}
        onCommit={(room_id) => onEdit({ ...target, room_id })}
      />
      <TextChip
        label="Token"
        title="Matrix access token"
        value={target.access_token}
        secret
        onCommit={(access_token) => onEdit({ ...target, access_token })}
      />
    </>
  );
}

function PostgresChips({
  target,
  onEdit,
}: {
  target: TargetOf<"postgres">;
  onEdit: (next: EventOutputTarget) => void;
}) {
  return (
    <>
      <TextChip
        label="Server"
        name="PostgreSQL URL"
        title="postgres://host:5432/database. Add ?sslmode=disable for a server without TLS."
        value={target.url}
        onCommit={(url) => onEdit({ ...target, url })}
      />
      <TextChip
        label="Table"
        name="PostgreSQL table"
        title="Created on first write. Lowercase letters, digits and _."
        value={target.table}
        onCommit={(table) => onEdit({ ...target, table })}
      />
      <TextChip
        label="User"
        title="PostgreSQL username"
        value={target.username ?? ""}
        onCommit={(username) => onEdit({ ...target, username })}
      />
      <TextChip
        label="Password"
        title="PostgreSQL password"
        value={target.password ?? ""}
        secret
        onCommit={(password) => onEdit({ ...target, password })}
      />
    </>
  );
}

function InfluxChips({
  target,
  onEdit,
}: {
  target: TargetOf<"influx">;
  onEdit: (next: EventOutputTarget) => void;
}) {
  return (
    <>
      <TextChip
        label="Server"
        name="InfluxDB URL"
        title="InfluxDB 2 or 3 base URL, e.g. http://127.0.0.1:8086"
        value={target.url}
        onCommit={(url) => onEdit({ ...target, url })}
      />
      <TextChip
        label="Bucket"
        name="InfluxDB bucket"
        title="Bucket, or database on InfluxDB 3"
        value={target.bucket}
        onCommit={(bucket) => onEdit({ ...target, bucket })}
      />
      <TextChip
        label="Org"
        title="InfluxDB organization"
        value={target.org ?? ""}
        onCommit={(org) => onEdit({ ...target, org })}
      />
      <TextChip
        label="Token"
        title="InfluxDB token"
        value={target.token ?? ""}
        secret
        onCommit={(token) => onEdit({ ...target, token })}
      />
    </>
  );
}

function MqttChips({
  target,
  onEdit,
}: {
  target: TargetOf<"mqtt">;
  onEdit: (next: EventOutputTarget) => void;
}) {
  return (
    <>
      <TextChip
        label="Broker"
        title="MQTT broker URL"
        value={target.broker_url}
        onCommit={(broker_url) => onEdit({ ...target, broker_url })}
      />
      <TextChip
        label="Topic"
        title="MQTT topic"
        value={target.topic}
        onCommit={(topic) => onEdit({ ...target, topic })}
      />
      <TextChip
        label="User"
        title="MQTT username"
        value={target.username ?? ""}
        onCommit={(username) => onEdit({ ...target, username })}
      />
      <TextChip
        label="Password"
        title="MQTT password"
        value={target.password ?? ""}
        secret
        onCommit={(password) => onEdit({ ...target, password })}
      />
    </>
  );
}
