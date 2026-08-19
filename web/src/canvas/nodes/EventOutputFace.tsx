import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { TextField } from "../../components/TextField";
import type { EventOutputTarget, PatchNode, PatchNodeOf } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import {
  eventOutputConfigured,
  newOutputTarget,
  OUTPUT_SERVICES,
  SERVICE_LABELS,
  WEBHOOK_FORMATS,
} from "./eventOutput";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

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
    <NodeShell
      node={node}
      title="Event output"
      category="sink"
      subtitle={SERVICE_LABELS[target.service]}
      live={inputs > 0 && configured}
    >
      <FaceBody>
        <Settings className="border-b border-line p-2">
          <SettingRow label="Service">
            <Select
              label="Output service"
              value={target.service}
              options={OUTPUT_SERVICES}
              onChange={(service) => {
                if (service !== target.service) {
                  editTarget(newOutputTarget(service));
                }
              }}
            />
          </SettingRow>
          <TargetFields target={target} onEdit={editTarget} />
        </Settings>
        <FaceEmpty>{emptyText(inputs, configured, target)}</FaceEmpty>
      </FaceBody>
    </NodeShell>
  );
}

function emptyText(inputs: number, configured: boolean, target: EventOutputTarget) {
  if (inputs === 0) {
    return "Wire an Events output from any decoder or DMR trunk into this sink.";
  }
  if (!configured) {
    return "Enter the destination credentials to start sending events.";
  }
  return carriesAudio(target)
    ? "Each event is sent once; completed calls include metadata and WAV audio."
    : "Each event is sent once, as one JSON object per decode.";
}

function carriesAudio(target: EventOutputTarget) {
  return (
    target.service === "matrix" || (target.service === "webhook" && target.format === "discord")
  );
}

function TargetFields({
  target,
  onEdit,
}: {
  target: EventOutputTarget;
  onEdit: (next: EventOutputTarget) => void;
}) {
  if (target.service === "webhook") {
    return (
      <>
        <SettingRow label="Endpoint">
          <TextField
            label="Webhook URL"
            value={target.url}
            secret
            onCommit={(url) => onEdit({ ...target, url })}
          />
        </SettingRow>
        <SettingRow label="Format">
          <Select
            label="Webhook payload format"
            value={target.format ?? "json"}
            options={WEBHOOK_FORMATS}
            onChange={(format) => onEdit({ ...target, format })}
          />
        </SettingRow>
      </>
    );
  }
  if (target.service === "matrix") {
    return (
      <>
        <SettingRow label="Homeserver">
          <TextField
            label="Matrix homeserver URL"
            value={target.homeserver_url}
            onCommit={(homeserver_url) => onEdit({ ...target, homeserver_url })}
          />
        </SettingRow>
        <SettingRow label="Room ID">
          <TextField
            label="Matrix room ID"
            value={target.room_id}
            onCommit={(room_id) => onEdit({ ...target, room_id })}
          />
        </SettingRow>
        <SettingRow label="Access token">
          <TextField
            label="Matrix access token"
            value={target.access_token}
            secret
            onCommit={(access_token) => onEdit({ ...target, access_token })}
          />
        </SettingRow>
      </>
    );
  }
  return (
    <>
      <SettingRow label="Broker">
        <TextField
          label="MQTT broker URL"
          value={target.broker_url}
          onCommit={(broker_url) => onEdit({ ...target, broker_url })}
        />
      </SettingRow>
      <SettingRow label="Topic">
        <TextField
          label="MQTT topic"
          value={target.topic}
          onCommit={(topic) => onEdit({ ...target, topic })}
        />
      </SettingRow>
      <SettingRow label="Username">
        <TextField
          label="MQTT username"
          value={target.username ?? ""}
          onCommit={(username) => onEdit({ ...target, username })}
        />
      </SettingRow>
      <SettingRow label="Password">
        <TextField
          label="MQTT password"
          value={target.password ?? ""}
          secret
          onCommit={(password) => onEdit({ ...target, password })}
        />
      </SettingRow>
    </>
  );
}
