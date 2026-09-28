import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

export function PendingFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const entry = workspace.context.catalog.nodes.find((type) => type.kind === node.kind);
  return (
    <NodeShell node={node} title={entry?.name ?? node.kind} category={entry?.category ?? "tool"}>
      <FaceBody>
        <FaceEmpty hint="Not built yet" />
      </FaceBody>
    </NodeShell>
  );
}
