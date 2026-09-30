import catalog from "../generated/patch-catalog.json";
import type { NodeBody, NodeKind, PatchCatalog } from "../lib/types";

export const CATALOG = catalog as unknown as PatchCatalog;

export function catalogBody(kind: NodeKind): NodeBody {
  const entry = CATALOG.nodes.find((type) => type.kind === kind);
  if (entry === undefined) {
    throw new Error(`the catalog has no ${kind}`);
  }
  return structuredClone(entry.default_body);
}
