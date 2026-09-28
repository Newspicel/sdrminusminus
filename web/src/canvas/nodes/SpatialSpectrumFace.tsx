import type { PatchNode } from "../../lib/types";
import { PendingFace } from "./PendingFace";

export function SpatialSpectrumFace({ node }: { node: PatchNode }) {
  return <PendingFace node={node} />;
}
