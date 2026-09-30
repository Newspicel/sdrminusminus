export interface Measurable {
  offsetTop: number;
  offsetHeight: number;
  scrollTop: number;
  offsetParent: Element | Measurable | null;
  parentElement: Element | Measurable | null;
}

function measurable(candidate: Element | Measurable | null): Measurable | null {
  return candidate !== null && "offsetTop" in candidate ? candidate : null;
}

export function offsetWithin(element: Measurable, container: Measurable | Element): number {
  let top = element.offsetTop + element.offsetHeight / 2;
  let parent = measurable(element.offsetParent);
  while (parent !== null && parent !== container) {
    top += parent.offsetTop;
    parent = measurable(parent.offsetParent);
  }
  let scroller = measurable(element.parentElement);
  while (scroller !== null && scroller !== container) {
    top -= scroller.scrollTop;
    scroller = measurable(scroller.parentElement);
  }
  return top;
}
