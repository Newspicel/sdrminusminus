import { describe, expect, it } from "vitest";
import { guardWheel } from "./wheel";

function setup() {
  const scope = new EventTarget();
  const area = new EventTarget();
  guardWheel(scope, area);
  let zooms = 0;
  scope.addEventListener("wheel", () => {
    zooms += 1;
  });
  const wheel = () => scope.dispatchEvent(new Event("wheel"));
  return { scope, area, wheel, zooms: () => zooms };
}

describe("guardWheel", () => {
  it("lets the page scroll until the demo is clicked", () => {
    const demo = setup();
    demo.wheel();
    expect(demo.zooms()).toBe(0);
    demo.scope.dispatchEvent(new Event("pointerdown"));
    demo.wheel();
    expect(demo.zooms()).toBe(1);
  });

  it("releases the wheel when the pointer leaves", () => {
    const demo = setup();
    demo.scope.dispatchEvent(new Event("pointerdown"));
    demo.area.dispatchEvent(new Event("mouseleave"));
    demo.wheel();
    expect(demo.zooms()).toBe(0);
  });

  it("releases the wheel when focus moves away", () => {
    const demo = setup();
    demo.scope.dispatchEvent(new Event("pointerdown"));
    demo.scope.dispatchEvent(new Event("blur"));
    demo.wheel();
    expect(demo.zooms()).toBe(0);
  });
});
