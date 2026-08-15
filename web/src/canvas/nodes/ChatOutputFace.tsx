import { useEffect, useState } from "react";
import { Input } from "../../components/BaseControls";
import { FIELD } from "../../components/controls";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import type { ChatOutputTarget, PatchNode, PatchNodeOf } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { CHAT_SERVICES, chatOutputConfigured } from "./chatOutput";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

export function ChatOutputFace({ node }: { node: PatchNode }) {
  if (node.kind !== "chat_output") {
    return null;
  }
  return <ChatOutputNodeFace node={node} />;
}

function ChatOutputNodeFace({ node }: { node: PatchNodeOf<"chat_output"> }) {
  const workspace = useWorkspaceContext();
  const target = node.data.target;
  const inputs = (workspace.graph.edges ?? []).filter(
    (edge) => edge.to.node === node.id && edge.to.port === "events",
  ).length;
  const configured = chatOutputConfigured(target);
  const editTarget = (next: ChatOutputTarget) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "chat_output" ? { ...current, data: { target: next } } : current,
      ),
    }));
  };
  return (
    <NodeShell
      node={node}
      title="Discord / Matrix"
      category="sink"
      subtitle={target.service === "discord" ? "Discord" : "Matrix"}
      live={inputs > 0 && configured}
    >
      <FaceBody>
        <Settings className="border-b border-line p-2">
          <SettingRow label="Service">
            <Select
              label="Chat service"
              value={target.service}
              options={CHAT_SERVICES}
              onChange={(service) => {
                if (service === target.service) {
                  return;
                }
                editTarget(
                  service === "discord"
                    ? { service: "discord", webhook_url: "" }
                    : {
                        service: "matrix",
                        homeserver_url: "",
                        room_id: "",
                        access_token: "",
                      },
                );
              }}
            />
          </SettingRow>
          {target.service === "discord" ? (
            <DraftField
              label="Webhook"
              ariaLabel="Discord webhook URL"
              value={target.webhook_url}
              secret
              onCommit={(webhook_url) => editTarget({ ...target, webhook_url })}
            />
          ) : (
            <>
              <DraftField
                label="Homeserver"
                ariaLabel="Matrix homeserver URL"
                value={target.homeserver_url}
                onCommit={(homeserver_url) => editTarget({ ...target, homeserver_url })}
              />
              <DraftField
                label="Room ID"
                ariaLabel="Matrix room ID"
                value={target.room_id}
                onCommit={(room_id) => editTarget({ ...target, room_id })}
              />
              <DraftField
                label="Access token"
                ariaLabel="Matrix access token"
                value={target.access_token}
                secret
                onCommit={(access_token) => editTarget({ ...target, access_token })}
              />
            </>
          )}
        </Settings>
        <FaceEmpty>
          {inputs === 0
            ? "Wire a DMR trunk system's events output into this sink."
            : configured
              ? "Each completed call is sent once with its metadata and WAV audio."
              : "Enter the destination credentials to start sending completed calls."}
        </FaceEmpty>
      </FaceBody>
    </NodeShell>
  );
}

function DraftField({
  label,
  ariaLabel,
  value,
  secret = false,
  onCommit,
}: {
  label: string;
  ariaLabel: string;
  value: string;
  secret?: boolean;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    const next = draft.trim();
    setDraft(next);
    if (next !== value) {
      onCommit(next);
    }
  };
  return (
    <SettingRow label={label}>
      <Input
        className={FIELD}
        aria-label={ariaLabel}
        type={secret ? "password" : "text"}
        autoComplete="off"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.currentTarget.blur();
          }
        }}
      />
    </SettingRow>
  );
}
