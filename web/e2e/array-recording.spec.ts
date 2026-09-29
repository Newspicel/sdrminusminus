import { type APIRequestContext, expect, test } from "@playwright/test";
import type {
  ArrayStatus,
  DeviceRef,
  RecordingInfo,
  RecordingsResponse,
  WorkspaceSnapshot,
} from "../src/lib/types";
import { face, laneWires, node, numbers, port, readout, stage, unstage, wire } from "./canvas";

test.describe.configure({ mode: "serial", timeout: 300_000 });
test.afterEach(({ request }) => unstage(request));

const KRAKEN: DeviceRef = { backend: "virtual", key: "kraken5" };
const LANES = 5;
const EMITTER_DEG = 137;
const LEAD_S = 1;
const TAIL_S = 3;
const SOLVE_MS = 60_000;
const SETTLE_MS = 90_000;

function site() {
  return node(
    "site",
    { kind: "gps", data: { source: { type: "fixed", lat: 52.0, lon: 13.0 } } },
    { x: -400, y: 0, w: 320, h: 300 },
  );
}

function array() {
  return node("arr", { kind: "array", data: {} }, { x: 440, y: 0, w: 460, h: 620 });
}

function bench(): WorkspaceSnapshot {
  return {
    version: 4,
    graph: {
      nodes: [
        node(
          "kraken",
          { kind: "device", data: { device: KRAKEN } },
          { x: 0, y: 0, w: 380, h: 700 },
        ),
        array(),
        site(),
      ],
      edges: [
        ...laneWires("kraken", "arr", LANES),
        wire(["site", "position"], ["arr", "position"]),
      ],
    },
  };
}

function replay(stem: string): WorkspaceSnapshot {
  return {
    version: 4,
    graph: {
      nodes: [
        node(
          "rec",
          { kind: "recording", data: { recording: stem } },
          { x: 0, y: 0, w: 420, h: 360 },
        ),
        array(),
        site(),
        node("finder", { kind: "df", data: {} }, { x: 960, y: 0, w: 380, h: 640 }),
      ],
      edges: [
        ...laneWires("rec", "arr", LANES),
        wire(["site", "position"], ["arr", "position"]),
        wire(["arr", "array"], ["finder", "array"]),
      ],
    },
  };
}

async function arrayStatus(request: APIRequestContext): Promise<ArrayStatus | null> {
  const statuses: ArrayStatus[] = await request.get("/api/arrays").then((r) => r.json());
  return statuses.find((status) => status.node === "arr") ?? null;
}

async function recorded(request: APIRequestContext): Promise<number> {
  return (await arrayStatus(request))?.recording?.samples ?? 0;
}

async function lastSolve(request: APIRequestContext): Promise<number> {
  const solved = (await arrayStatus(request))?.last_solve_at;
  return solved == null ? 0 : Date.parse(solved);
}

async function library(request: APIRequestContext): Promise<RecordingInfo[]> {
  const listed: RecordingsResponse = await request.get("/api/recordings").then((r) => r.json());
  return listed.recordings;
}

test("records an array and finds the bearing again from the collection", async ({ page }) => {
  await stage(page, "Array take", bench());
  const bank = face(page, "arr");
  await expect(readout(bank, "Cal")).toHaveText(/^Calibrated/, { timeout: SOLVE_MS });
  const rate = (await arrayStatus(page.request))?.sample_rate ?? 0;
  expect(rate).toBeGreaterThan(0);

  await bank.getByRole("button", { name: "Rec" }).click();
  await expect
    .poll(() => recorded(page.request), { timeout: SOLVE_MS })
    .toBeGreaterThan(LEAD_S * rate);
  const asked = Date.now();
  await bank.getByRole("button", { name: "Calibrate" }).click();
  await expect.poll(() => lastSolve(page.request), { timeout: SOLVE_MS }).toBeGreaterThan(asked);
  const solved = await recorded(page.request);
  await expect
    .poll(() => recorded(page.request), { timeout: SOLVE_MS })
    .toBeGreaterThan(solved + TAIL_S * rate);
  const stem = (await arrayStatus(page.request))?.recording?.stem ?? "";
  expect(stem).not.toBe("");
  await bank.getByRole("button", { name: "Stop" }).click();

  await expect
    .poll(async () =>
      (await library(page.request))
        .filter((entry) => entry.file.startsWith(stem))
        .map((entry) => [entry.file, entry.lanes]),
    )
    .toEqual([[stem, LANES]]);
  await page.getByRole("button", { name: "Library" }).click();
  await page.getByRole("tab", { name: "Recordings" }).click();
  await expect(page.getByText(stem, { exact: true })).toBeVisible();
  await expect(page.getByText(`${stem}-lane0`)).toHaveCount(0);
  await expect(page.getByText(/^5 lanes · /)).toBeVisible();

  await stage(page, "Array replay", replay(stem));
  const played = face(page, "rec");
  await expect(readout(played, "Lanes")).toHaveText(String(LANES));
  await expect(port(played, "iq5")).toHaveCount(1);
  const finder = face(page, "finder");
  await expect
    .poll(
      async () => {
        const [read] = numbers(await readout(finder, "Bearing").textContent());
        return read === undefined ? Number.POSITIVE_INFINITY : Math.abs(read - EMITTER_DEG);
      },
      { timeout: SETTLE_MS },
    )
    .toBeLessThan(3);
  await expect(finder.locator("header")).toContainText("5 lanes");
});
