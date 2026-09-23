import { AudioControls } from "../../components/AudioControls";
import { audioChainActive } from "../../components/channelSettings";
import { Settings } from "../../components/Settings";
import type { AudioProcessing, PatchNode, PatchNodeOf } from "../../lib/types";
import { sourcesOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";

export function AudioFxFace({ node }: { node: PatchNode }) {
  if (node.kind !== "audio_fx") {
    return null;
  }
  return <Face node={node} />;
}

function Face({ node }: { node: PatchNodeOf<"audio_fx"> }) {
  const workspace = useWorkspaceContext();
  const audio: AudioProcessing = node.data?.settings ?? {};
  const wired = sourcesOf(workspace.graph, node.id, "audio").length > 0;

  const edit = (settings: AudioProcessing) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "audio_fx" ? { ...current, data: { ...current.data, settings } } : current,
      ),
    }));
  };

  return (
    <NodeShell
      node={node}
      title="Audio FX"
      category="tool"
      subtitle={audioChainActive(audio) ? "on" : undefined}
    >
      <FaceBody title={wired ? undefined : "Wire channel audio in"}>
        <Settings className="p-3">
          <AudioControls audio={audio} onAudio={edit} />
        </Settings>
      </FaceBody>
    </NodeShell>
  );
}
