import type { WorkspaceExport, WorkspaceSnapshot } from "../lib/types";
import { migrateSnapshot } from "./graph";

export const WORKSPACE_FILE_ACCEPT = ".json,application/json";

export function parseWorkspaceExport(text: string): WorkspaceExport {
  let document: unknown;
  try {
    document = JSON.parse(text);
  } catch {
    throw new Error("That file is not a workspace: it is not JSON.");
  }
  if (!isRecord(document)) {
    throw new Error("That file is not a workspace.");
  }
  const { version, name, snapshot } = document;
  if (
    typeof version !== "number" ||
    typeof name !== "string" ||
    name.trim() === "" ||
    !isRecord(snapshot) ||
    !isRecord(snapshot.graph) ||
    !Array.isArray(snapshot.graph.nodes)
  ) {
    throw new Error("That file is not a workspace.");
  }
  return {
    ...(document as unknown as WorkspaceExport),
    snapshot: migrateSnapshot(snapshot as unknown as WorkspaceSnapshot),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
