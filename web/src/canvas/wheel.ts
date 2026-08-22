export interface WheelGesture {
  ctrlKey: boolean;
  metaKey: boolean;
  deltaX: number;
  deltaY: number;
}

export interface WheelBox {
  overflowX: string;
  overflowY: string;
  scrollWidth: number;
  clientWidth: number;
  scrollHeight: number;
  clientHeight: number;
}

export function movesCanvas(gesture: WheelGesture): boolean {
  return gesture.ctrlKey || gesture.metaKey;
}

export function boxScrolls(box: WheelBox, gesture: WheelGesture): boolean {
  const vertical = Math.abs(gesture.deltaY) >= Math.abs(gesture.deltaX);
  const overflow = vertical ? box.overflowY : box.overflowX;
  if (overflow !== "auto" && overflow !== "scroll") {
    return false;
  }
  return vertical ? box.scrollHeight > box.clientHeight : box.scrollWidth > box.clientWidth;
}

export function wheelStaysOnFace(event: WheelEvent, face: HTMLElement): boolean {
  if (movesCanvas(event)) {
    return false;
  }
  let element = event.target instanceof HTMLElement ? event.target : null;
  while (element !== null) {
    if (boxScrolls(boxOf(element), event)) {
      return true;
    }
    element = element === face ? null : element.parentElement;
  }
  return false;
}

function boxOf(element: HTMLElement): WheelBox {
  const room =
    element.scrollHeight > element.clientHeight || element.scrollWidth > element.clientWidth;
  const style = room ? getComputedStyle(element) : null;
  return {
    overflowX: style?.overflowX ?? "visible",
    overflowY: style?.overflowY ?? "visible",
    scrollWidth: element.scrollWidth,
    clientWidth: element.clientWidth,
    scrollHeight: element.scrollHeight,
    clientHeight: element.clientHeight,
  };
}
