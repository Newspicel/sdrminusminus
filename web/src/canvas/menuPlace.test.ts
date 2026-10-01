import { expect, it } from "vitest";
import { menuPlace } from "./menuPlace";

const VIEW = { width: 1280, height: 720 };
const MENU = { width: 208, height: 132 };

it("opens the menu where the pointer is when it fits", () => {
  expect(menuPlace({ left: 40, top: 100 }, MENU, VIEW)).toEqual({ left: 40, top: 100 });
});

it("lifts a menu opened at the bottom edge back into the window", () => {
  expect(menuPlace({ left: 40, top: 680 }, MENU, VIEW)).toEqual({ left: 40, top: 584 });
});

it("pulls a menu opened at the right edge back into the window", () => {
  expect(menuPlace({ left: 1270, top: 10 }, MENU, VIEW)).toEqual({ left: 1068, top: 10 });
});

it("keeps the top left corner on screen when the window is too small", () => {
  expect(menuPlace({ left: 50, top: 50 }, MENU, { width: 100, height: 100 })).toEqual({
    left: 4,
    top: 4,
  });
});
