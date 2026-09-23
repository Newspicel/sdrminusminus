import { expect, type Page } from "@playwright/test";
import type {
  ChannelInfo,
  ChannelSettings,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  PatchEdge,
  PatchNode,
  RackSlot,
  StateSnapshot,
  WorkspaceSnapshot,
} from "../src/lib/types";

const SIGGEN: DeviceRef = { backend: "virtual", key: "siggen" };

export interface Scene {
  id: string;
  title: string;
  stage(page: Page): Promise<void>;
  ready(page: Page): Promise<void>;
  speaker?: string;
  settleSeconds: number;
}

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

function channel(id: string, type: string, box: Box): PatchNode {
  return node(id, { kind: "channel", data: { channel_type: type } }, box);
}

function wire(from: [string, string], to: [string, string]): PatchEdge {
  return { from: { node: from[0], port: from[1] }, to: { node: to[0], port: to[1] } };
}

function slot(id: string, cell: Box): RackSlot {
  return { node: id, x: cell.x, y: cell.y, w: cell.w, h: cell.h };
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
  await expect(page.getByRole("button", { name: "Add a node" })).toBeVisible();
}

async function deviceSet(page: Page, device: DeviceRef): Promise<DeviceSet> {
  const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
  const set = state.device_sets.find((candidate) => candidate.device.key === device.key);
  if (set === undefined) {
    throw new Error(`an open device set for ${device.key}`);
  }
  return set;
}

async function amend(
  page: Page,
  device: DeviceRef,
  type: string,
  change: (open: ChannelInfo) => Partial<ChannelSettings>,
): Promise<void> {
  const set = await deviceSet(page, device);
  for (const open of set.channels) {
    if (open.settings.params.type !== type) {
      continue;
    }
    const response = await page.request.patch(`/api/devicesets/${set.id}/channels/${open.id}`, {
      data: { ...open.settings, ...change(open) },
    });
    if (!response.ok()) {
      throw new Error(`setting channel ${open.id}: ${await response.text()}`);
    }
  }
}

async function tune(page: Page, device: DeviceRef, offsets: Record<string, number>): Promise<void> {
  const set = await deviceSet(page, device);
  const centerHz = set.settings.center_hz ?? 0;
  for (const [type, offset] of Object.entries(offsets)) {
    await amend(page, device, type, () => ({ frequency_hz: centerHz + offset }));
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

async function showRack(page: Page): Promise<void> {
  await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Rack" }).click();
}

export async function addNode(page: Page, name: string): Promise<void> {
  await page.getByRole("button", { name: "Add a node" }).click();
  await page.getByRole("textbox", { name: "Search nodes" }).fill(name);
  await page.getByRole("button", { name, exact: true }).click();
}

export function face(page: Page, id: string) {
  return page.locator(`.react-flow__node[data-id="${id}"]`);
}

export async function listen(page: Page, id: string): Promise<void> {
  const shell = face(page, id);
  await shell.click();
  await shell.getByRole("button", { name: /^play$/i }).click();
}

function siggenPatch(): WorkspaceSnapshot {
  return {
    version: 3,
    graph: {
      nodes: [
        node("dev", { kind: "device", data: { device: SIGGEN } }, { x: 0, y: 0, w: 380, h: 420 }),
        node("scope", { kind: "scope" }, { x: 440, y: 0, w: 800, h: 420 }),
        channel("ch", "nfm", { x: 0, y: 480, w: 560, h: 620 }),
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

const patch: Scene = {
  id: "patch",
  title: "one radio feeding several channels",
  speaker: "speaker",
  settleSeconds: 10,
  async stage(page) {
    await stage(page, "Signal generator", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device: SIGGEN } }, { x: 0, y: 0, w: 380, h: 222 }),
          node("scope", { kind: "scope" }, { x: 440, y: 0, w: 1400, h: 440 }),
          channel("nfm", "nfm", { x: 0, y: 520, w: 440, h: 541 }),
          channel("am", "am", { x: 480, y: 520, w: 440, h: 445 }),
          channel("wfm", "wfm", { x: 960, y: 520, w: 440, h: 469 }),
          node("speaker", { kind: "speaker" }, { x: 1440, y: 520, w: 320, h: 270 }),
          node(
            "rec",
            { kind: "audio_recorder", data: { recording: false } },
            { x: 1440, y: 830, w: 340, h: 70 },
          ),
          node("udp", { kind: "network_export", data: {} }, { x: 1440, y: 940, w: 380, h: 240 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["nfm", "iq"]),
          wire(["dev", "iq"], ["am", "iq"]),
          wire(["dev", "iq"], ["wfm", "iq"]),
          wire(["nfm", "audio"], ["speaker", "audio"]),
          wire(["wfm", "audio"], ["rec", "audio"]),
          wire(["am", "baseband"], ["udp", "baseband"]),
        ],
      },
    });
    await tune(page, SIGGEN, { nfm: 300_000, am: -300_000, wfm: 600_000 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(face(page, "scope").getByText(/MHz/).first()).toBeVisible();
  },
};

const spectrum: Scene = {
  id: "spectrum",
  title: "the spectrum and waterfall",
  settleSeconds: 16,
  async stage(page) {
    const snapshot = siggenPatch();
    snapshot.rack = { slots: [slot("scope", { x: 0, y: 0, w: 12, h: 8 })] };
    await stage(page, "Spectrum", snapshot);
    await tune(page, SIGGEN, { nfm: 300_000 });
    await showRack(page);
    const scope = page.locator('.grid > [data-id="scope"]');
    await expect(scope).toBeVisible();
    await scope.getByRole("button", { name: "Scope settings" }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: /^viridis$/i })
      .click();
    await page.keyboard.press("Escape");
    await page.mouse.move(0, 0);
  },
  async ready(page) {
    await expect(page.locator('.grid > [data-id="scope"]').getByText(/MHz/).first()).toBeVisible();
  },
};

const adsb: Scene = {
  id: "adsb",
  title: "aircraft on the map",
  settleSeconds: 8,
  async stage(page) {
    const device = await recording(page, "adsb_squitters_2m");
    await stage(page, "Aircraft (ADS-B)", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "adsb", { x: 0, y: 350, w: 440, h: 190 }),
          node("scope", { kind: "scope" }, { x: 480, y: 0, w: 720, h: 540 }),
          node("map", { kind: "map" }, { x: 1240, y: 0, w: 700, h: 540 }),
          node("log", { kind: "decoder_log" }, { x: 0, y: 600, w: 1940, h: 560 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["map", "events"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { adsb: 0 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(face(page, "map").getByText("Aircraft")).toBeVisible();
    await expect(
      face(page, "log")
        .getByText(/DLH123/)
        .first(),
    ).toBeVisible({ timeout: 60_000 });
  },
};

const ais: Scene = {
  id: "ais",
  title: "ships on the map",
  settleSeconds: 8,
  async stage(page) {
    const device = await recording(page, "ais_position_240k");
    await stage(page, "Ships (AIS)", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "ais", { x: 0, y: 350, w: 440, h: 204 }),
          node("scope", { kind: "scope" }, { x: 0, y: 600, w: 440, h: 300 }),
          node("map", { kind: "map" }, { x: 480, y: 0, w: 1020, h: 720 }),
          node("log", { kind: "decoder_log" }, { x: 480, y: 760, w: 1020, h: 140 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["map", "events"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { ais: 25_000 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(
      face(page, "log")
        .getByText(/211234560/)
        .first(),
    ).toBeVisible({ timeout: 60_000 });
  },
};

const sstv: Scene = {
  id: "sstv",
  title: "an SSTV picture in the readout",
  settleSeconds: 2,
  async stage(page) {
    const device = await recording(page, "sstv_robot36_48k");
    await stage(page, "SSTV", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "sstv", { x: 0, y: 350, w: 440, h: 300 }),
          node("readout", { kind: "readout" }, { x: 500, y: 0, w: 1200, h: 560 }),
          node("scope", { kind: "scope" }, { x: 0, y: 610, w: 1700, h: 300 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["readout", "events"]),
        ],
      },
    });
    await tune(page, device, { sstv: 4_000 });
    await fitPatch(page);
  },
  async ready(page) {
    const readout = face(page, "readout");
    await expect(readout.getByRole("img", { name: /picture received/i })).toBeVisible({
      timeout: 120_000,
    });
    await expect(readout.getByText("complete")).toBeVisible({ timeout: 120_000 });
  },
};

const pocsag: Scene = {
  id: "pocsag",
  title: "decoded pager traffic",
  settleSeconds: 10,
  async stage(page) {
    const device = await recording(page, "pocsag_1200_240k");
    await stage(page, "Pagers (POCSAG)", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "pocsag", { x: 0, y: 350, w: 440, h: 262 }),
          node(
            "hook",
            {
              kind: "event_output",
              data: {
                target: {
                  service: "webhook",
                  url: "https://dispatch.example.org/pocsag",
                  format: "json",
                },
              },
            },
            { x: 0, y: 680, w: 420, h: 209 },
          ),
          node("scope", { kind: "scope" }, { x: 480, y: 0, w: 1240, h: 360 }),
          node("log", { kind: "decoder_log" }, { x: 480, y: 410, w: 1240, h: 620 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["log", "events"]),
          wire(["ch", "events"], ["hook", "events"]),
        ],
      },
    });
    await tune(page, device, { pocsag: 50_000 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(
      face(page, "log")
        .getByText(/SDR-- FIXTURE/)
        .first(),
    ).toBeVisible({ timeout: 60_000 });
  },
};

const ft8: Scene = {
  id: "ft8",
  title: "a busy FT8 slot",
  settleSeconds: 4,
  async stage(page) {
    const device = await recording(page, "ft8_20m_busy_12k");
    await stage(page, "Weak signal (FT8)", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "ft8", { x: 0, y: 350, w: 440, h: 274 }),
          node("scope", { kind: "scope" }, { x: 480, y: 0, w: 1180, h: 300 }),
          node("log", { kind: "decoder_log" }, { x: 480, y: 340, w: 1180, h: 620 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { ft8: 0 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(face(page, "log").getByText(/OH8JK/).first()).toBeVisible({ timeout: 180_000 });
  },
};

const rds: Scene = {
  id: "rds",
  title: "broadcast FM with RDS",
  speaker: "speaker",
  settleSeconds: 8,
  async stage(page) {
    const device = await recording(page, "rds_station_960k");
    await stage(page, "Broadcast FM (RDS)", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "wfm", { x: 0, y: 350, w: 440, h: 469 }),
          node("speaker", { kind: "speaker" }, { x: 0, y: 860, w: 320, h: 210 }),
          node("scope", { kind: "scope" }, { x: 480, y: 0, w: 1200, h: 760 }),
          node("readout", { kind: "readout" }, { x: 480, y: 800, w: 1200, h: 270 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "audio"], ["speaker", "audio"]),
          wire(["ch", "events"], ["readout", "events"]),
        ],
      },
    });
    await tune(page, device, { wfm: 200_000 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(
      face(page, "readout")
        .getByText(/SDR-M4/)
        .first(),
    ).toBeVisible({ timeout: 60_000 });
  },
};

const ident: Scene = {
  id: "ident",
  title: "an unknown signal identified",
  settleSeconds: 6,
  async stage(page) {
    const device = await recording(page, "pocsag_1200_240k");
    await stage(page, "Signal identification", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "ident", { x: 0, y: 350, w: 440, h: 274 }),
          node("readout", { kind: "readout" }, { x: 480, y: 0, w: 1180, h: 470 }),
          node("scope", { kind: "scope" }, { x: 0, y: 680, w: 1660, h: 300 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["readout", "events"]),
        ],
      },
    });
    await tune(page, device, { ident: 0 });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(
      face(page, "readout")
        .getByText(/confident/i)
        .first(),
    ).toBeVisible({ timeout: 60_000 });
  },
};

const atv: Scene = {
  id: "atv",
  title: "analogue television",
  settleSeconds: 24,
  async stage(page) {
    const device = await recording(page, "atv_ccir625_2m4");
    await stage(page, "Amateur television", {
      version: 3,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 380, h: 290 }),
          channel("ch", "atv", { x: 0, y: 350, w: 440, h: 639 }),
          node("scope", { kind: "scope" }, { x: 480, y: 0, w: 1180, h: 400 }),
          node("video", { kind: "video" }, { x: 480, y: 440, w: 1180, h: 548 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "video"], ["video", "video"]),
        ],
      },
    });
    await tune(page, device, { atv: 200_000 });
    await amend(page, device, "atv", (open) => {
      const params = open.settings.params;
      if (params.type !== "atv") {
        throw new Error("an ATV channel");
      }
      return { params: { ...params, settings: { ...params.settings, interlace: false } } };
    });
    await fitPatch(page);
  },
  async ready(page) {
    await expect(face(page, "video").locator("canvas")).toBeVisible({ timeout: 60_000 });
  },
};

const rack: Scene = {
  id: "rack",
  title: "three receivers in one rack",
  settleSeconds: 6,
  async stage(page) {
    const aisDevice = await recording(page, "ais_position_240k");
    const pocsagDevice = await recording(page, "pocsag_1200_240k");
    const sstvDevice = await recording(page, "sstv_robot36_48k");
    await stage(page, "Watch desk", {
      version: 3,
      graph: {
        nodes: [
          node(
            "sea",
            { kind: "device", data: { device: aisDevice } },
            { x: 0, y: 0, w: 380, h: 290 },
          ),
          node(
            "pag",
            { kind: "device", data: { device: pocsagDevice } },
            { x: 0, y: 340, w: 380, h: 290 },
          ),
          node(
            "pic",
            { kind: "device", data: { device: sstvDevice } },
            { x: 0, y: 680, w: 380, h: 290 },
          ),
          channel("ch_ais", "ais", { x: 440, y: 0, w: 440, h: 204 }),
          channel("ch_pocsag", "pocsag", { x: 440, y: 340, w: 440, h: 262 }),
          channel("ch_sstv", "sstv", { x: 440, y: 680, w: 440, h: 300 }),
          node("scope", { kind: "scope" }, { x: 940, y: 0, w: 700, h: 300 }),
          node("map", { kind: "map" }, { x: 940, y: 340, w: 700, h: 300 }),
          node("log", { kind: "decoder_log" }, { x: 940, y: 680, w: 700, h: 300 }),
          node("readout", { kind: "readout" }, { x: 1700, y: 0, w: 700, h: 300 }),
        ],
        edges: [
          wire(["sea", "iq"], ["scope", "iq"]),
          wire(["sea", "iq"], ["ch_ais", "iq"]),
          wire(["pag", "iq"], ["ch_pocsag", "iq"]),
          wire(["pic", "iq"], ["ch_sstv", "iq"]),
          wire(["ch_ais", "events"], ["map", "events"]),
          wire(["ch_pocsag", "events"], ["log", "events"]),
          wire(["ch_sstv", "events"], ["readout", "events"]),
        ],
      },
      rack: {
        slots: [
          slot("scope", { x: 0, y: 0, w: 7, h: 4 }),
          slot("map", { x: 7, y: 0, w: 5, h: 4 }),
          slot("log", { x: 0, y: 4, w: 7, h: 4 }),
          slot("readout", { x: 7, y: 4, w: 5, h: 4 }),
        ],
      },
    });
    await tune(page, aisDevice, { ais: 25_000 });
    await tune(page, pocsagDevice, { pocsag: 50_000 });
    await tune(page, sstvDevice, { sstv: 4_000 });
    await showRack(page);
  },
  async ready(page) {
    const readout = page.locator('.grid > [data-id="readout"]');
    await expect(readout.getByRole("img", { name: /picture received/i })).toBeVisible({
      timeout: 180_000,
    });
    await page.mouse.move(0, 0);
  },
};

export const SCENES: Scene[] = [
  patch,
  spectrum,
  adsb,
  ais,
  sstv,
  pocsag,
  ft8,
  rds,
  ident,
  atv,
  rack,
];

export function scene(id: string): Scene {
  const found = SCENES.find((candidate) => candidate.id === id);
  if (found === undefined) {
    throw new Error(`no scene ${id}`);
  }
  return found;
}
