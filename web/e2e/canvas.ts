import { type APIRequestContext, expect, type Locator, type Page } from "@playwright/test";
import type {
  DeviceRef,
  PatchEdge,
  PatchNode,
  StateSnapshot,
  WorkspaceSnapshot,
} from "../src/lib/types";

const WIRE_ATTEMPTS = 3;

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function node(id: string, body: Record<string, unknown>, box: Box): PatchNode {
  return {
    id,
    position: { x: box.x, y: box.y },
    size: { w: box.w, h: box.h },
    ...body,
  } as PatchNode;
}

export function wire(from: [string, string], to: [string, string]): PatchEdge {
  return { from: { node: from[0], port: from[1] }, to: { node: to[0], port: to[1] } };
}

export function streamPort(base: string, index: number): string {
  return index === 0 ? base : `${base}${index + 1}`;
}

export function laneWires(device: string, array: string, lanes: number): PatchEdge[] {
  return Array.from({ length: lanes }, (_, lane) =>
    wire([device, streamPort("iq", lane)], [array, streamPort("lane", lane)]),
  );
}

export function edgeKey(from: [string, string], to: [string, string]): string {
  return `${from[0]}.${from[1]}->${to[0]}.${to[1]}`;
}

export async function stage(
  page: Page,
  name: string,
  snapshot: WorkspaceSnapshot,
): Promise<number> {
  const response = await page.request.post("/api/workspaces", { data: { name, snapshot } });
  const created: { id?: number; error?: string } = await response.json();
  if (created.id === undefined) {
    throw new Error(`workspace ${name} was rejected: ${created.error ?? response.status()}`);
  }
  await page.request.post(`/api/workspaces/${created.id}/activate`);
  const report = await page.request.post(`/api/workspaces/${created.id}/apply`);
  expect(report.ok()).toBe(true);
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Add a node" })).toBeVisible();
  return created.id;
}

export async function deviceSetOf(request: APIRequestContext, device: DeviceRef): Promise<number> {
  const state: StateSnapshot = await request.get("/api/state").then((r) => r.json());
  const set = state.device_sets.find((candidate) => candidate.device.key === device.key);
  if (set === undefined) {
    throw new Error(`an open device set for ${device.key}`);
  }
  return set.id;
}

export async function setExtra(
  request: APIRequestContext,
  deviceSet: number,
  name: string,
  value: number,
): Promise<void> {
  const response = await request.patch(`/api/devicesets/${deviceSet}/device`, {
    data: { extra: [{ name, value }] },
  });
  expect(response.ok(), await response.text()).toBe(true);
}

export async function dropWire(page: Page, from: Locator, to: Locator): Promise<void> {
  const start = await from.boundingBox();
  const end = await to.boundingBox();
  if (start === null || end === null) {
    throw new Error("a port to wire from and one to wire to");
  }
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(end.x + end.width / 2, end.y + end.height / 2, { steps: 12 });
  await page.mouse.up();
}

export async function dragWire(page: Page, from: Locator, to: Locator): Promise<void> {
  const wires = page.locator(".react-flow__edge");
  for (let attempt = 0; attempt < WIRE_ATTEMPTS; attempt++) {
    const before = await wires.count();
    await dropWire(page, from, to);
    const landed = await expect(wires)
      .toHaveCount(before + 1, { timeout: 2_000 })
      .then(() => true)
      .catch(() => false);
    if (landed) {
      return;
    }
  }
  throw new Error("the wire never landed");
}

export async function deleteWire(page: Page, key: string): Promise<void> {
  const edge = page.locator(`.react-flow__edge[data-id="${key}"]`);
  const exposed = await edge
    .locator("path.react-flow__edge-path")
    .evaluate((path: SVGPathElement, id: string) => {
      const screen = path.getScreenCTM();
      if (screen === null) {
        return null;
      }
      const length = path.getTotalLength();
      const steps = 40;
      for (let step = 0; step < steps; step++) {
        const offset = (step % 2 === 0 ? 1 : -1) * Math.ceil(step / 2);
        const along = path.getPointAtLength((length * (steps / 2 + offset)) / steps);
        const at = new DOMPoint(along.x, along.y).matrixTransform(screen);
        const hit = document.elementFromPoint(at.x, at.y);
        if (hit?.closest(".react-flow__edge")?.getAttribute("data-id") === id) {
          return { x: at.x, y: at.y };
        }
      }
      return null;
    }, key);
  if (exposed === null) {
    throw new Error(`a visible stretch of the wire ${key}`);
  }
  await page.mouse.click(exposed.x, exposed.y, { button: "right" });
  await page.getByRole("menu").getByRole("button", { name: "Delete wire" }).click();
  await expect(edge).toHaveCount(0);
}

export async function viewSettled(page: Page): Promise<void> {
  const viewport = page.locator(".react-flow__viewport");
  const transform = () => viewport.evaluate((element) => (element as HTMLElement).style.transform);
  await expect
    .poll(async () => {
      const before = await transform();
      await page.waitForTimeout(150);
      return before === (await transform());
    })
    .toBe(true);
}

export async function fitPatch(page: Page): Promise<void> {
  const pane = page.locator(".react-flow__pane");
  const box = await pane.boundingBox();
  if (box === null) {
    throw new Error("a pane to right-click");
  }
  await expect(page.locator('.react-flow__node[style*="visibility: hidden"]')).toHaveCount(0);
  await page.mouse.click(box.x + 40, box.y + box.height - 40, { button: "right" });
  await page
    .getByRole("menu")
    .getByRole("button", { name: /fit the patch/i })
    .click();
  await viewSettled(page);
}

export async function leaveField(shell: Locator): Promise<void> {
  await shell.locator("header").click();
}

export async function activate(shell: Locator): Promise<void> {
  await shell.locator("header").click();
}

export async function addNode(page: Page, name: string): Promise<void> {
  await page.getByRole("button", { name: "Add a node" }).click();
  const search = page.getByRole("textbox", { name: "Search nodes" });
  await search.fill(name);
  const palette = search.locator("xpath=ancestor::div[nav][1]");
  await palette.getByRole("button", { name, exact: true }).click();
}

export function face(page: Page, id: string): Locator {
  return page.locator(`.react-flow__node[data-id="${id}"]`);
}

export function port(shell: Locator, name: string): Locator {
  return shell.locator(`.react-flow__handle[data-handleid="${name}"]`);
}

export function readout(scope: Locator, label: string): Locator {
  return scope
    .locator("span.legend")
    .filter({ hasText: new RegExp(`^${label}$`) })
    .locator("xpath=following-sibling::span[1]");
}

export function numbers(text: string | null): number[] {
  return (text ?? "").match(/-?\d+(\.\d+)?/g)?.map(Number) ?? [];
}
