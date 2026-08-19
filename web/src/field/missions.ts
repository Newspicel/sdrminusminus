import type { ComponentType } from "react";
import type { NodeKind, PatchGraph } from "../lib/types";

export const FIELD_ROOT = "/field";

export interface MissionProps {
  node: string;
}

export interface Mission {
  id: string;
  title: string;
  blurb: string;
  /// The kind of patch node this mission drives. Nothing else can be picked for it.
  nodeKind: NodeKind;
  component: ComponentType<MissionProps>;
}

/// Where in the field-mode address the mission and its node sit: `/field/<mission>/<node>`, with
/// `/field` on its own being the picker.
export interface FieldRoute {
  mission: string | null;
  node: string | null;
}

export function parseFieldPath(pathname: string): FieldRoute {
  const parts = pathname
    .replace(/^\/+|\/+$/g, "")
    .split("/")
    .filter((part) => part.length > 0);
  if (parts[0] !== "field") {
    return { mission: null, node: null };
  }
  return {
    mission: parts[1] ?? null,
    node: parts[2] === undefined ? null : decodeURIComponent(parts[2]),
  };
}

export function fieldPath(mission: string, node: string): string {
  return `${FIELD_ROOT}/${mission}/${encodeURIComponent(node)}`;
}

export function isFieldPath(pathname: string): boolean {
  return pathname === FIELD_ROOT || pathname.startsWith(`${FIELD_ROOT}/`);
}

/// Every node in the patch a mission could be run against.
export function missionTargets(
  graph: PatchGraph,
  missions: readonly Mission[],
): { mission: Mission; node: string; label: string }[] {
  const found: { mission: Mission; node: string; label: string }[] = [];
  for (const mission of missions) {
    for (const node of graph.nodes) {
      if (node.kind !== mission.nodeKind) {
        continue;
      }
      found.push({ mission, node: node.id, label: node.label ?? node.id });
    }
  }
  return found;
}
