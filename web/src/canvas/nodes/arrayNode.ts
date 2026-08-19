import type { ArrayNode, DeviceInfo } from "../../lib/types";

export const MAX_ARRAY_MEMBERS = 16;

export interface ArrayMember {
  id: string;
  label: string;
  attached: boolean;
}

/// The array's members in lane order, named by whatever is plugged in. A member that is not on the
/// bus keeps its place: the lane numbering is the operator's wiring, not what happens to be found.
export function arrayMembers(settings: ArrayNode, attached: readonly DeviceInfo[]): ArrayMember[] {
  return settings.members.map((id) => {
    const device = attached.find((candidate) => `${candidate.driver}:${candidate.key}` === id);
    return {
      id,
      label: device?.label ?? `${id} (not connected)`,
      attached: device !== undefined,
    };
  });
}

export function withMember(members: readonly string[], id: string): string[] {
  return members.includes(id) || members.length >= MAX_ARRAY_MEMBERS
    ? [...members]
    : [...members, id];
}

export function withoutMember(members: readonly string[], id: string): string[] {
  return members.filter((member) => member !== id);
}

export function moveMember(members: readonly string[], index: number, by: number): string[] {
  const next = [...members];
  const to = index + by;
  const moved = next[index];
  const displaced = next[to];
  if (moved === undefined || displaced === undefined) {
    return next;
  }
  next[index] = displaced;
  next[to] = moved;
  return next;
}
