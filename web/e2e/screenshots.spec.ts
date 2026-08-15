import { expect, type Page, test } from "@playwright/test";
import type {
  DeviceInfo,
  DeviceRef,
  PatchEdge,
  PatchNode,
  StateSnapshot,
  WorkspaceSnapshot,
} from "../src/lib/types";

const SHOTS = "../assets/screenshots";
const SIGGEN: DeviceRef = { backend: "virtual", key: "siggen" };

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

function node(id: string, body: Record<string, unknown>, box: Box): PatchNode {
  return {
    id,
    position: { x: box.x, y: box.y },
    size: { w: box.w, h: box.h },
    ...body,
  } as PatchNode;
}

function wire(from: [string, string], to: [string, string]): PatchEdge {
  return { from: { node: from[0], port: from[1] }, to: { node: to[0], port: to[1] } };
}

async function recording(page: Page, stem: string): Promise<DeviceRef> {
  const body: { devices: DeviceInfo[] } = await page.request
    .get("/api/devices")
    .then((response) => response.json());
  const found = body.devices.find((device) => device.label.startsWith(stem));
  if (found === undefined) {
    throw new Error(`a recording device for ${stem}`);
  }
  return { backend: found.driver, key: found.key };
}

async function stage(page: Page, name: string, snapshot: WorkspaceSnapshot): Promise<void> {
  const response = await page.request.post("/api/workspaces", { data: { name, snapshot } });
  const created: { id?: number; error?: string } = await response.json();
  if (created.id === undefined) {
    throw new Error(`workspace ${name} was rejected: ${created.error ?? response.status()}`);
  }
  await page.request.post(`/api/workspaces/${created.id}/activate`);
  const report = await page.request.post(`/api/workspaces/${created.id}/apply`);
  expect(report.ok()).toBe(true);
  await page.goto("/");
  await expect(page.getByRole("button", { name: "+ Node" })).toBeVisible();
}

async function tuneChannels(page: Page, device: DeviceRef, offsetHz: number): Promise<void> {
  const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
  const set = state.device_sets.find((candidate) => candidate.device.key === device.key);
  if (set === undefined) {
    throw new Error(`an open device set for ${device.key}`);
  }
  for (const channel of set.channels) {
    const settings = { ...channel.settings, offset_hz: offsetHz };
    const response = await page.request.patch(`/api/devicesets/${set.id}/channels/${channel.id}`, {
      data: settings,
    });
    if (!response.ok()) {
      throw new Error(`tuning channel ${channel.id}: ${await response.text()}`);
    }
  }
}

async function fitPatch(page: Page): Promise<void> {
  const pane = page.locator(".react-flow__pane");
  const box = await pane.boundingBox();
  if (box === null) {
    throw new Error("a pane to right-click");
  }
  await page.mouse.click(box.x + 40, box.y + box.height - 40, { button: "right" });
  await page
    .getByRole("menu")
    .getByRole("button", { name: /fit the patch/i })
    .click();
  await page.keyboard.press("Escape");
  await page.mouse.click(box.x + 12, box.y + 12);
  await page.mouse.move(0, 0);
  await page
    .locator("body")
    .evaluate((element) => (element.ownerDocument.activeElement as HTMLElement | null)?.blur());
}

async function capture(page: Page, name: string, settleSeconds: number): Promise<void> {
  await page.waitForTimeout(settleSeconds * 1000);
  await page.screenshot({ path: `${SHOTS}/${name}.png` });
}

function siggenPatch(): WorkspaceSnapshot {
  return {
    version: 2,
    graph: {
      nodes: [
        node("dev", { kind: "device", data: { device: SIGGEN } }, { x: 0, y: 0, w: 380, h: 420 }),
        node("scope", { kind: "scope" }, { x: 440, y: 0, w: 800, h: 420 }),
        node(
          "ch",
          { kind: "channel", data: { channel_type: "nfm" } },
          { x: 0, y: 480, w: 560, h: 620 },
        ),
        node("speaker", { kind: "speaker" }, { x: 620, y: 480, w: 620, h: 300 }),
      ],
      edges: [
        wire(["dev", "iq"], ["scope", "iq"]),
        wire(["dev", "iq"], ["ch", "iq"]),
        wire(["ch", "audio"], ["speaker", "audio"]),
      ],
    },
  };
}

test("the patch editor", async ({ page }) => {
  await page.goto("/");
  await stage(page, "Signal generator", siggenPatch());
  await tuneChannels(page, SIGGEN, 300_000);
  await fitPatch(page);

  const scope = page.locator('.react-flow__node[data-id="scope"]');
  await expect(scope).toBeVisible();
  await expect(scope.getByText(/waiting for the first frame/i)).toHaveCount(0);
  await capture(page, "patch", 8);
});

test("the spectrum and waterfall", async ({ page }) => {
  await page.goto("/");
  const snapshot = siggenPatch();
  snapshot.rack = { slots: [{ node: "scope", x: 0, y: 0, w: 12, h: 8 }] };
  await stage(page, "Spectrum", snapshot);
  await tuneChannels(page, SIGGEN, 300_000);

  await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Rack" }).click();
  const scope = page.locator('.grid > [data-id="scope"]');
  await expect(scope).toBeVisible();
  await expect(scope.getByText(/waiting for the first frame/i)).toHaveCount(0);

  await scope.getByRole("button", { name: /^classic$/i }).click();
  await page.getByRole("button", { name: /^viridis$/i }).click();
  await page.keyboard.press("Escape");
  await page.mouse.move(0, 0);
  await capture(page, "spectrum", 16);
});

test("aircraft on the map", async ({ page }) => {
  await page.goto("/");
  const device = await recording(page, "adsb_squitters_2m");
  await stage(page, "Aircraft (ADS-B)", {
    version: 2,
    graph: {
      nodes: [
        node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 420 }),
        node("scope", { kind: "scope" }, { x: 440, y: 0, w: 700, h: 420 }),
        node("map", { kind: "map" }, { x: 1200, y: 0, w: 700, h: 420 }),
        node(
          "ch",
          { kind: "channel", data: { channel_type: "adsb" } },
          { x: 0, y: 480, w: 560, h: 460 },
        ),
        node("log", { kind: "decoder_log" }, { x: 620, y: 480, w: 1280, h: 460 }),
      ],
      edges: [
        wire(["dev", "iq"], ["scope", "iq"]),
        wire(["dev", "iq"], ["ch", "iq"]),
        wire(["ch", "events"], ["map", "events"]),
        wire(["ch", "events"], ["log", "events"]),
      ],
    },
  });
  await tuneChannels(page, device, 0);
  await fitPatch(page);

  const map = page.locator('.react-flow__node[data-id="map"]');
  await expect(map.getByText("Aircraft")).toBeVisible();
  await expect(
    page
      .locator('.react-flow__node[data-id="log"]')
      .getByText(/DLH123/)
      .first(),
  ).toBeVisible({ timeout: 60_000 });
  await capture(page, "adsb", 8);
});

test("an SSTV picture in the readout", async ({ page }) => {
  await page.goto("/");
  const device = await recording(page, "sstv_robot36_48k");
  await stage(page, "SSTV", {
    version: 2,
    graph: {
      nodes: [
        node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 420 }),
        node("scope", { kind: "scope" }, { x: 440, y: 0, w: 640, h: 420 }),
        node("readout", { kind: "readout" }, { x: 1140, y: 0, w: 760, h: 620 }),
        node(
          "ch",
          { kind: "channel", data: { channel_type: "sstv" } },
          { x: 0, y: 480, w: 1080, h: 600 },
        ),
      ],
      edges: [
        wire(["dev", "iq"], ["scope", "iq"]),
        wire(["dev", "iq"], ["ch", "iq"]),
        wire(["ch", "events"], ["readout", "events"]),
      ],
    },
  });
  await tuneChannels(page, device, 4_000);
  await fitPatch(page);

  const readout = page.locator('.react-flow__node[data-id="readout"]');
  await expect(readout.getByRole("img", { name: /picture received/i })).toBeVisible({
    timeout: 120_000,
  });
  await expect(readout.getByText("complete")).toBeVisible({ timeout: 120_000 });
  await capture(page, "sstv", 2);
});

test("decoded pager traffic", async ({ page }) => {
  await page.goto("/");
  const device = await recording(page, "pocsag_1200_240k");
  await stage(page, "Pagers (POCSAG)", {
    version: 2,
    graph: {
      nodes: [
        node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 420 }),
        node("scope", { kind: "scope" }, { x: 440, y: 0, w: 640, h: 420 }),
        node("log", { kind: "decoder_log" }, { x: 1140, y: 0, w: 900, h: 420 }),
        node(
          "ch",
          { kind: "channel", data: { channel_type: "pocsag" } },
          { x: 0, y: 480, w: 1080, h: 520 },
        ),
      ],
      edges: [
        wire(["dev", "iq"], ["scope", "iq"]),
        wire(["dev", "iq"], ["ch", "iq"]),
        wire(["ch", "events"], ["log", "events"]),
      ],
    },
  });
  await tuneChannels(page, device, 50_000);
  await fitPatch(page);

  await expect(
    page
      .locator('.react-flow__node[data-id="log"]')
      .getByText(/SDR-- FIXTURE/)
      .first(),
  ).toBeVisible({ timeout: 60_000 });
  await capture(page, "pocsag", 10);
});
