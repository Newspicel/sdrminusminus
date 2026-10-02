import { expect, type Locator, type Page, test } from "@playwright/test";
import type { WorkspaceDetail, WorkspacesResponse } from "../src/lib/types";
import { face, fitPatch, node, stage, unstage } from "./canvas";

const GRID = 24;

async function stored(page: Page): Promise<number[]> {
  const listed: WorkspacesResponse = await page.request
    .get("/api/workspaces")
    .then((r) => r.json());
  const detail: WorkspaceDetail = await page.request
    .get(`/api/workspaces/${listed.active}`)
    .then((r) => r.json());
  const scope = detail.snapshot.graph.nodes.find((item) => item.id === "scope");
  return [
    scope?.position.x ?? Number.NaN,
    scope?.position.y ?? Number.NaN,
    scope?.size?.w ?? 0,
    scope?.size?.h ?? 0,
  ];
}

function onGrid(values: number[]): boolean {
  return values.every((value) => Math.abs(value % GRID) < 0.5);
}

async function drag(page: Page, grip: Locator, dx: number, dy: number): Promise<void> {
  const box = await grip.boundingBox();
  if (box === null) {
    throw new Error("a grip to drag");
  }
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + dx, y + dy, { steps: 8 });
  await page.mouse.up();
}

test.beforeEach(async ({ page }) => {
  await stage(page, "Grid snapping", {
    version: 4,
    graph: {
      nodes: [node("scope", { kind: "scope" }, { x: 0, y: 0, w: 480, h: 312 })],
      edges: [],
    },
  });
  await fitPatch(page);
});

test.afterEach(({ request }) => unstage(request));

test("dragging and resizing land on the grid", async ({ page }) => {
  const scope = face(page, "scope");
  const before = await stored(page);

  await drag(page, scope.locator("header"), 37, 53);
  await expect.poll(async () => (await stored(page)).slice(0, 2)).not.toEqual(before.slice(0, 2));
  expect(onGrid((await stored(page)).slice(0, 2))).toBe(true);

  await scope.locator("header").click();
  const moved = await stored(page);
  await drag(page, scope.locator(".react-flow__resize-control.handle.bottom.right"), 41, 29);
  await expect.poll(async () => (await stored(page)).slice(2)).not.toEqual(moved.slice(2));
  expect(onGrid(await stored(page))).toBe(true);
});
