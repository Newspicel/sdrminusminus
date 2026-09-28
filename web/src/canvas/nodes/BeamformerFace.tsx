import type { PatchNode } from "../../lib/types";
import { PendingFace } from "./PendingFace";

export function BeamformerFace({ node }: { node: PatchNode }) {
  return <PendingFace node={node} />;
}
