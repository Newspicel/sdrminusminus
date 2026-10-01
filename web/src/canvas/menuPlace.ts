const EDGE_PX = 4;

export interface MenuPlace {
  left: number;
  top: number;
}

export function menuPlace(
  at: MenuPlace,
  size: { width: number; height: number },
  view: { width: number; height: number },
): MenuPlace {
  return {
    left: within(at.left, size.width, view.width),
    top: within(at.top, size.height, view.height),
  };
}

function within(start: number, extent: number, room: number): number {
  return Math.max(EDGE_PX, Math.min(start, room - extent - EDGE_PX));
}
