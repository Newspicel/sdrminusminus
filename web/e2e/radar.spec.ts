import { expect, type Page, test } from "@playwright/test";
import type { DeviceRef, WorkspaceSnapshot } from "../src/lib/types";
import {
  deleteWire,
  edgeKey,
  face,
  fitPatch,
  laneWires,
  node,
  stage,
  unstage,
  wire,
} from "./canvas";

const KRAKEN: DeviceRef = { backend: "virtual", key: "kraken5" };
const LANES = 5;
const TRACK_MS = 60_000;
const QUIET_MS = 10_000;

test.describe.configure({ timeout: 180_000 });
test.afterEach(({ request }) => unstage(request));

function fixed(id: string, lat: number, lon: number, x: number, y: number) {
  return node(
    id,
    { kind: "gps", data: { source: { type: "fixed", lat, lon } } },
    { x, y, w: 320, h: 300 },
  );
}

function radarBench(): WorkspaceSnapshot {
  return {
    version: 4,
    graph: {
      nodes: [
        node(
          "kraken",
          { kind: "device", data: { device: KRAKEN } },
          { x: 0, y: 0, w: 380, h: 700 },
        ),
        node("arr", { kind: "array", data: {} }, { x: 440, y: 0, w: 460, h: 620 }),
        fixed("rx", 52.0, 13.0, -400, 0),
        fixed("tx", 52.1, 13.2, 440, 720),
        node("radar", { kind: "passive_radar", data: {} }, { x: 960, y: 0, w: 640, h: 760 }),
        node("map", { kind: "map" }, { x: 1660, y: 0, w: 720, h: 560 }),
      ],
      edges: [
        ...laneWires("kraken", "arr", LANES),
        wire(["rx", "position"], ["arr", "position"]),
        wire(["arr", "array"], ["radar", "array"]),
        wire(["tx", "position"], ["radar", "tx"]),
        wire(["radar", "events"], ["map", "events"]),
      ],
    },
  };
}

function countSubscribes(page: Page): () => number {
  let subscribes = 0;
  page.on("websocket", (socket) => {
    socket.on("framesent", (frame) => {
      if (typeof frame.payload === "string" && frame.payload.includes("SubscribeSurface")) {
        subscribes += 1;
      }
    });
  });
  return () => subscribes;
}

test("draws range and Doppler with axes, tracks and echoes on the map", async ({ page }) => {
  const subscribes = countSubscribes(page);
  await stage(page, "Radar bench", radarBench());
  const radar = face(page, "radar");
  const map = face(page, "map");
  await fitPatch(page);

  for (const unit of ["km", "Hz", "m/s", "dB"]) {
    await expect(radar.getByText(unit, { exact: true }).first()).toBeVisible();
  }
  await expect
    .poll(() => radar.locator("table tbody tr").count(), { timeout: TRACK_MS })
    .toBeGreaterThan(0);
  await page.waitForTimeout(QUIET_MS);
  expect(subscribes()).toBe(1);
  await expect(map.getByText("Echoes", { exact: true })).toBeVisible();
  await expect(map.getByText("No position", { exact: true })).toHaveCount(0);

  await deleteWire(page, edgeKey(["tx", "position"], ["radar", "tx"]));
  await expect(radar.locator("header")).toContainText("no tx");
  await expect(map.getByText("No position", { exact: true })).toBeVisible();
});
