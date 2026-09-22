import type { PatchNode } from "../../lib/types";
import { basebandSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { BasebandView } from "./BasebandView";
import { FaceBody, NodeShell } from "./NodeShell";
import { readColormap } from "./ScopeFace";

export function BasebandScopeFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const tap = basebandSourceOf(
    workspace.graph,
    node.id,
    workspace.devices,
    workspace.channels,
    workspace.owners,
  );
  return (
    <NodeShell
      node={node}
      title="Baseband scope"
      category="output"
      subtitle={tap === null ? undefined : `${tap.channel.settings.params.type} baseband`}
    >
      <FaceBody scroll={false}>
        {tap === null ? (
          <div className="flex h-full items-center justify-center bg-plot-bg legend text-plot-ink-dim">
            Wire a channel's baseband
          </div>
        ) : (
          <BasebandView
            key={`${tap.deviceSet}:${tap.channel.id}`}
            deviceSet={tap.deviceSet}
            channel={tap.channel}
            colormap={readColormap()}
          />
        )}
      </FaceBody>
    </NodeShell>
  );
}
