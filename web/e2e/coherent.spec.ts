import { type APIRequestContext, expect, type Locator, type Page, test } from "@playwright/test";
import type { ArrayStatus, DeviceRef, WorkspaceSnapshot } from "../src/lib/types";
import {
  addNode,
  deleteWire,
  deviceSetOf,
  dragWire,
  dropWire,
  edgeKey,
  face,
  fitPatch,
  laneWires,
  node,
  numbers,
  port,
  readout,
  setExtra,
  stage,
  wire,
} from "./canvas";

test.describe.configure({ mode: "serial", timeout: 180_000 });

const KRAKEN: DeviceRef = { backend: "virtual", key: "kraken5" };
const LANES = 5;
const BEARING = "wavefront_bearing_deg";
const EMITTER_DEG = 137;
const FIRST_DEG = 148.3;
const SECOND_DEG = 211.7;
const CROSSING = { lat: 51.95, lon: 13.05 };
const TUNED_HZ = 145_000_000;
const SOLVE_MS = 30_000;
const SETTLE_MS = 60_000;

function kraken(x: number, y: number) {
  return node("kraken", { kind: "device", data: { device: KRAKEN } }, { x, y, w: 380, h: 700 });
}

function site(lon: number, x: number, y: number) {
  return node(
    "site",
    { kind: "gps", data: { source: { type: "fixed", lat: 52.0, lon } } },
    { x, y, w: 320, h: 300 },
  );
}

function bench(): WorkspaceSnapshot {
  return { version: 4, graph: { nodes: [kraken(0, 0), site(13.0, -400, 0)], edges: [] } };
}

function crossings(): WorkspaceSnapshot {
  return {
    version: 4,
    graph: {
      nodes: [
        kraken(0, 0),
        node("arr", { kind: "array", data: {} }, { x: 440, y: 0, w: 460, h: 620 }),
        site(13.0, -400, 0),
        node("finder", { kind: "df", data: {} }, { x: 960, y: 0, w: 380, h: 640 }),
        node("tri", { kind: "triangulation", data: {} }, { x: 1400, y: 0, w: 400, h: 720 }),
        node("map", { kind: "map" }, { x: 960, y: 700, w: 840, h: 520 }),
      ],
      edges: [
        ...laneWires("kraken", "arr", LANES),
        wire(["site", "position"], ["arr", "position"]),
        wire(["arr", "array"], ["finder", "array"]),
        wire(["finder", "events"], ["tri", "events"]),
        wire(["tri", "events"], ["map", "events"]),
        wire(["finder", "events"], ["map", "events"]),
      ],
    },
  };
}

function finding(): WorkspaceSnapshot {
  const { nodes, edges } = crossings().graph;
  return {
    version: 4,
    graph: {
      nodes: nodes.filter((placed) => placed.id !== "tri" && placed.id !== "map"),
      edges: (edges ?? []).filter((edge) => edge.to.node !== "tri" && edge.to.node !== "map"),
    },
  };
}

function nodeOf(page: Page, kind: string): Locator {
  return page.locator(`.react-flow__node[data-id^="${kind}:"]`);
}

async function idOf(located: Locator): Promise<string> {
  const id = await located.getAttribute("data-id");
  if (id === null) {
    throw new Error("a node with an id");
  }
  return id;
}

async function bearingNear(finder: Locator, deg: number, within: number): Promise<void> {
  await expect
    .poll(
      async () => {
        const [read] = numbers(await readout(finder, "Bearing").textContent());
        return read === undefined ? Number.POSITIVE_INFINITY : Math.abs(read - deg);
      },
      { timeout: SETTLE_MS },
    )
    .toBeLessThan(within);
}

async function lastSolve(request: APIRequestContext, array: string): Promise<number> {
  const statuses: ArrayStatus[] = await request.get("/api/arrays").then((r) => r.json());
  const solved = statuses.find((status) => status.node === array)?.last_solve_at;
  return solved == null ? 0 : Date.parse(solved);
}

async function estimateError(tri: Locator): Promise<number> {
  const [lat, lon] = numbers(await readout(tri, "Estimate").textContent());
  if (lat === undefined || lon === undefined) {
    return Number.POSITIVE_INFINITY;
  }
  return Math.max(Math.abs(lat - CROSSING.lat), Math.abs(lon - CROSSING.lon));
}

test("makes an array from a Kraken and finds a bearing", async ({ page }) => {
  await stage(page, "Kraken bench", bench());
  const radio = face(page, "kraken");
  const make = radio.getByRole("button", { name: "Make array" });
  await expect(make).toBeEnabled({ timeout: SOLVE_MS });
  await make.click();

  const array = nodeOf(page, "array");
  await expect(array).toHaveCount(1);
  const arrayId = await idOf(array);
  await expect(
    page.locator(`.react-flow__edge[data-id^="kraken."][data-id*="->${arrayId}.lane"]`),
  ).toHaveCount(LANES);
  await expect(array.getByRole("table", { name: "Lanes" }).locator("tbody tr")).toHaveCount(LANES);

  await fitPatch(page);
  await dragWire(page, port(face(page, "site"), "position"), port(array, "position"));

  const asked = Date.now();
  await array.getByRole("button", { name: "Calibrate" }).click();
  await expect
    .poll(() => lastSolve(page.request, arrayId), { timeout: SOLVE_MS })
    .toBeGreaterThan(asked);
  await expect(readout(array, "Cal")).toHaveText(/^Calibrated/);

  const tuned = await page.request.patch(`/api/arrays/${encodeURIComponent(arrayId)}/tune`, {
    data: { center_hz: TUNED_HZ },
  });
  expect(tuned.status(), await tuned.text()).toBe(204);
  await expect(array.getByRole("spinbutton", { name: "Tuned frequency" })).toHaveAttribute(
    "aria-valuetext",
    /^145\.000/,
  );

  await addNode(page, "Direction finder");
  const finder = nodeOf(page, "df");
  await expect(finder).toHaveCount(1);
  await fitPatch(page);
  await dragWire(page, port(array, "array"), port(finder, "array"));
  await bearingNear(finder, EMITTER_DEG, 3);
  await expect(finder.locator("header")).toContainText("5 lanes");
});

test("crosses bearings from two places and clears them", async ({ page }) => {
  await stage(page, "Two places", crossings());
  const deviceSet = await deviceSetOf(page.request, KRAKEN);
  const finder = face(page, "finder");
  const tri = face(page, "tri");
  const map = face(page, "map");
  await fitPatch(page);

  await setExtra(page.request, deviceSet, BEARING, FIRST_DEG);
  await bearingNear(finder, FIRST_DEG, 1.5);
  const cleared = await page.request.delete("/api/fusion/tri");
  expect(cleared.status()).toBe(204);
  await expect
    .poll(async () => numbers(await readout(tri, "Bearings").textContent())[0] ?? 0, {
      timeout: SETTLE_MS,
    })
    .toBeGreaterThanOrEqual(6);

  const longitude = face(page, "site").getByRole("textbox", { name: "Longitude" });
  await longitude.fill("13.1");
  await longitude.press("Enter");
  await setExtra(page.request, deviceSet, BEARING, SECOND_DEG);
  await expect.poll(() => estimateError(tri), { timeout: SETTLE_MS }).toBeLessThan(0.01);

  await expect(map.getByText("Bearings", { exact: true })).toBeVisible();
  await expect(map.getByText("Heat", { exact: true })).toBeVisible();
  await expect(map.getByText("No position", { exact: true })).toHaveCount(0);

  await deleteWire(page, edgeKey(["finder", "events"], ["tri", "events"]));
  await tri.getByRole("button", { name: "Clear" }).click();
  await expect(readout(tri, "Estimate")).toHaveText("-");
  await expect(readout(tri, "Bearings")).toHaveText("0");
});

test("refuses a lane already in another array on the face", async ({ page }) => {
  await stage(page, "Two arrays", crossings());
  await addNode(page, "Array");
  const second = nodeOf(page, "array");
  await expect(second).toHaveCount(1);
  await fitPatch(page);
  await dropWire(page, port(face(page, "kraken"), "iq2"), port(second, "lane"));
  await expect(second.getByRole("alert")).toContainText("that lane is in");
  await expect(
    page.locator(`.react-flow__edge[data-id="kraken.iq2->${await idOf(second)}.lane"]`),
  ).toHaveCount(0);
});

function textFrames(page: Page): string[] {
  const frames: string[] = [];
  page.on("websocket", (socket) =>
    socket.on("framereceived", (frame) => {
      if (typeof frame.payload === "string") {
        frames.push(frame.payload);
      }
    }),
  );
  return frames;
}

function refusedSurfaces(frames: readonly string[]): string[] {
  return frames.filter((frame) => frame.includes("no surface"));
}

test("a triangulation added by hand shows its heat", async ({ page }) => {
  const frames = textFrames(page);
  await stage(page, "Hand triangulation", finding());
  await addNode(page, "Triangulation");
  const tri = nodeOf(page, "triangulation");
  await expect(tri).toHaveCount(1);
  await fitPatch(page);
  await dragWire(
    page,
    port(face(page, "finder"), "events"),
    tri.locator('.react-flow__handle.target[data-handleid="events"]'),
  );
  await expect(tri.getByRole("img", { name: "Bearing heat" }).locator("canvas")).toHaveCount(1, {
    timeout: SETTLE_MS,
  });
  expect(refusedSurfaces(frames)).toEqual([]);
});

test("a spatial spectrum added by hand streams its surface", async ({ page }) => {
  const frames = textFrames(page);
  await stage(page, "Hand spatial spectrum", finding());
  await addNode(page, "Spatial spectrum");
  const spatial = nodeOf(page, "spatial_spectrum");
  await expect(spatial).toHaveCount(1);
  const id = await idOf(spatial);
  await fitPatch(page);
  await dragWire(
    page,
    port(face(page, "arr"), "array"),
    spatial.locator('.react-flow__handle.target[data-handleid="array"]'),
  );
  await expect
    .poll(
      () =>
        frames.some((frame) => frame.includes("SurfaceStreamStarted") && frame.includes(`"${id}"`)),
      { timeout: SETTLE_MS },
    )
    .toBe(true);
  expect(refusedSurfaces(frames)).toEqual([]);
});

async function arrayStatus(request: APIRequestContext, array: string): Promise<ArrayStatus | null> {
  const statuses: ArrayStatus[] = await request.get("/api/arrays").then((r) => r.json());
  return statuses.find((status) => status.node === array) ?? null;
}

async function processorsOn(request: APIRequestContext, array: string): Promise<string[]> {
  return ((await arrayStatus(request, array))?.processors ?? []).map((held) => held.node);
}

test("steps the array dial once per key press, even between status updates", async ({ page }) => {
  await stage(page, "Array dial", finding());
  const dial = face(page, "arr").getByRole("spinbutton", { name: "Tuned frequency" });
  await expect(dial).not.toHaveAttribute("aria-disabled", "true", { timeout: SOLVE_MS });
  const start = (await arrayStatus(page.request, "arr"))?.center_hz ?? 0;
  await dial.focus();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await expect
    .poll(async () => (await arrayStatus(page.request, "arr"))?.center_hz, { timeout: SETTLE_MS })
    .toBe(start + 3_000_000);
  await expect(dial).toHaveAttribute("aria-valuenow", String(start + 3_000_000));
});

test("removing a finder on its face stops it on the server", async ({ page }) => {
  await stage(page, "Finder removal", finding());
  await expect
    .poll(() => processorsOn(page.request, "arr"), { timeout: SOLVE_MS })
    .toContain("finder");
  await face(page, "finder").getByRole("button", { name: "Remove Direction finder" }).click();
  await expect(face(page, "finder")).toHaveCount(0);
  await expect
    .poll(() => processorsOn(page.request, "arr"), { timeout: SETTLE_MS })
    .not.toContain("finder");
});
