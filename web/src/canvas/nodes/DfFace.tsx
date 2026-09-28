import type { PatchNode } from "../../lib/types";
import { PendingFace } from "./PendingFace";

export function DfFace({ node }: { node: PatchNode }) {
  return <PendingFace node={node} />;
}
