import type { PatchNode } from "../../lib/types";
import { PendingFace } from "./PendingFace";

export function CorrelatorFace({ node }: { node: PatchNode }) {
  return <PendingFace node={node} />;
}
