import { MAX_NAME_LEN } from "./graph";

export const DEFAULT_WORKSPACE_NAME = "Workspace";

export function workspaceName(typed: string): string {
  const trimmed = typed.trim().slice(0, MAX_NAME_LEN).trimEnd();
  return trimmed === "" ? DEFAULT_WORKSPACE_NAME : trimmed;
}
