import { expect, type Page } from "@playwright/test";
import type {
  ChannelInfo,
  ChannelSettings,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  PatchNode,
  RackSlot,
  StateSnapshot,
  WorkspaceSnapshot,
} from "../src/lib/types";
import { type Box, face, fitPatch, node, stage, wire } from "./canvas";

const SIGGEN: DeviceRef = { backend: "virtual", key: "band" };
const SIGGEN_CENTER_HZ = 100_000_000;

export interface Scene {
  id: string;
  title: string;
  stage(page: Page): Promise<void>;
  ready(page: Page): Promise<void>;
  speaker?: string;
  settleSeconds: number;
}

function channel(id: string, type: string, box: Box): PatchNode {
  return node(id, { kind: "channel", data: { channel_type: type } }, box);
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
  const centerHz = device === SIGGEN ? SIGGEN_CENTER_HZ : (set.settings.center_hz ?? 0);
  for (const [type, offset] of Object.entries(offsets)) {
    await amend(page, device, type, () => ({ frequency_hz: centerHz + offset }));
  }
  if (set.settings.center_hz === centerHz) {
    return;
  }
  const response = await page.request.patch(`/api/devicesets/${set.id}/device`, {
    data: { ...set.settings, center_hz: centerHz },
  });
  if (!response.ok()) {
    throw new Error(`centering ${device.key}: ${await response.text()}`);
  }
}

async function fitForCapture(page: Page): Promise<void> {
  await fitPatch(page);
  await page.keyboard.press("Escape");
  const box = await page.locator(".react-flow__pane").boundingBox();
  if (box === null) {
    throw new Error("a pane to click");
  }
  await page.mouse.click(box.x + 12, box.y + 12);
  await idle(page);
}

async function idle(page: Page): Promise<void> {
  await page.mouse.move(0, 0);
  await page
    .locator("body")
    .evaluate((element) => (element.ownerDocument.activeElement as HTMLElement | null)?.blur());
}

async function showRack(page: Page): Promise<void> {
  await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Rack" }).click();
}

export async function listen(page: Page, id: string): Promise<void> {
  const shell = face(page, id);
  await shell.click();
  await shell.getByRole("button", { name: /^play$/i }).click();
}

function siggenPatch(): WorkspaceSnapshot {
  return {
    version: 4,
    graph: {
      nodes: [
        node("dev", { kind: "device", data: { device: SIGGEN } }, { x: 0, y: 0, w: 420, h: 236 }),
        channel("ch", "nfm", { x: 540, y: 0, w: 460, h: 331 }),
        node("scope", { kind: "scope" }, { x: 540, y: 371, w: 880, h: 400 }),
        node("speaker", { kind: "speaker" }, { x: 1140, y: 0, w: 280, h: 204 }),
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
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device: SIGGEN } }, { x: 0, y: 0, w: 420, h: 236 }),
          node("scope", { kind: "scope" }, { x: 1140, y: 0, w: 760, h: 400 }),
          channel("nfm", "nfm", { x: 540, y: 100, w: 460, h: 331 }),
          channel("am", "am", { x: 540, y: 471, w: 460, h: 229 }),
          channel("wfm", "wfm", { x: 540, y: 740, w: 460, h: 255 }),
          node("speaker", { kind: "speaker" }, { x: 1140, y: 440, w: 280, h: 204 }),
          node("udp", { kind: "network_export", data: {} }, { x: 1140, y: 684, w: 280, h: 242 }),
          node(
            "rec",
            { kind: "audio_recorder", data: { recording: false } },
            { x: 1140, y: 966, w: 280, h: 104 },
          ),
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
    await fitForCapture(page);
  },
  async ready(page) {
    await expect(face(page, "scope").getByText(/MHz/).first()).toBeVisible();
  },
};

const spectrum: Scene = {
  id: "spectrum",
  title: "the spectrum and waterfall",
  settleSeconds: 26,
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
    await idle(page);
  },
  async ready(page) {
    await expect(page.locator('.grid > [data-id="scope"]').getByText(/MHz/).first()).toBeVisible();
  },
};

const adsb: Scene = {
  id: "adsb",
  title: "aircraft on the map",
  settleSeconds: 20,
  async stage(page) {
    const device = await recording(page, "adsb_squitters_2m");
    await stage(page, "Aircraft (ADS-B)", {
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "adsb", { x: 540, y: 0, w: 420, h: 141 }),
          node("scope", { kind: "scope" }, { x: 540, y: 181, w: 420, h: 819 }),
          node("map", { kind: "map" }, { x: 1100, y: 0, w: 800, h: 560 }),
          node("log", { kind: "decoder_log" }, { x: 1100, y: 600, w: 800, h: 400 }),
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
    await fitForCapture(page);
  },
  async ready(page) {
    await expect(face(page, "map").getByText("Aircraft")).toBeVisible();
    await expect(
      face(page, "log")
        .getByText(/DLH123/)
        .first(),
    ).toBeVisible({ timeout: 60_000 });
    await face(page, "map").getByRole("button", { name: "Zoom out" }).click();
    await idle(page);
  },
};

const ais: Scene = {
  id: "ais",
  title: "ships on the map",
  settleSeconds: 16,
  async stage(page) {
    const device = await recording(page, "ais_position_240k");
    await stage(page, "Ships (AIS)", {
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "ais", { x: 540, y: 0, w: 420, h: 153 }),
          node("scope", { kind: "scope" }, { x: 540, y: 193, w: 420, h: 807 }),
          node("map", { kind: "map" }, { x: 1100, y: 0, w: 800, h: 760 }),
          node("log", { kind: "decoder_log" }, { x: 1100, y: 800, w: 800, h: 200 }),
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
    await fitForCapture(page);
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
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "sstv", { x: 540, y: 0, w: 420, h: 218 }),
          node("scope", { kind: "scope" }, { x: 540, y: 258, w: 420, h: 582 }),
          node("readout", { kind: "readout" }, { x: 1100, y: 0, w: 800, h: 530 }),
          node("log", { kind: "decoder_log" }, { x: 1100, y: 570, w: 800, h: 270 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["readout", "events"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { sstv: 4_000 });
    await fitForCapture(page);
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
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "pocsag", { x: 540, y: 0, w: 420, h: 217 }),
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
            { x: 1100, y: 803, w: 280, h: 197 },
          ),
          node("scope", { kind: "scope" }, { x: 540, y: 257, w: 420, h: 743 }),
          node("log", { kind: "decoder_log" }, { x: 1100, y: 0, w: 800, h: 763 }),
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
    await fitForCapture(page);
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
  settleSeconds: 10,
  async stage(page) {
    const device = await recording(page, "ft8_20m_busy_12k");
    await stage(page, "Weak signal (FT8)", {
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "ft8", { x: 540, y: 0, w: 420, h: 229 }),
          node("scope", { kind: "scope" }, { x: 540, y: 269, w: 420, h: 731 }),
          node("log", { kind: "decoder_log" }, { x: 1100, y: 0, w: 800, h: 1000 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { ft8: 0 });
    await fitForCapture(page);
  },
  async ready(page) {
    await expect(face(page, "log").getByText(/OH8JK/).first()).toBeVisible({ timeout: 180_000 });
  },
};

const rds: Scene = {
  id: "rds",
  title: "broadcast FM with RDS",
  speaker: "speaker",
  settleSeconds: 16,
  async stage(page) {
    const device = await recording(page, "rds_station_960k");
    await stage(page, "Broadcast FM (RDS)", {
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "wfm", { x: 540, y: 0, w: 460, h: 255 }),
          node("scope", { kind: "scope" }, { x: 540, y: 295, w: 460, h: 705 }),
          node("speaker", { kind: "speaker" }, { x: 1140, y: 0, w: 280, h: 204 }),
          node("readout", { kind: "readout" }, { x: 1140, y: 244, w: 760, h: 220 }),
          node("log", { kind: "decoder_log" }, { x: 1140, y: 504, w: 760, h: 496 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "audio"], ["speaker", "audio"]),
          wire(["ch", "events"], ["readout", "events"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { wfm: 200_000 });
    await fitForCapture(page);
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
  settleSeconds: 14,
  async stage(page) {
    const device = await recording(page, "pocsag_1200_240k");
    await stage(page, "Signal identification", {
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          channel("ch", "ident", { x: 540, y: 0, w: 420, h: 229 }),
          node("scope", { kind: "scope" }, { x: 540, y: 269, w: 420, h: 731 }),
          node("readout", { kind: "readout" }, { x: 1100, y: 0, w: 800, h: 560 }),
          node("log", { kind: "decoder_log" }, { x: 1100, y: 600, w: 800, h: 400 }),
        ],
        edges: [
          wire(["dev", "iq"], ["scope", "iq"]),
          wire(["dev", "iq"], ["ch", "iq"]),
          wire(["ch", "events"], ["readout", "events"]),
          wire(["ch", "events"], ["log", "events"]),
        ],
      },
    });
    await tune(page, device, { ident: 0 });
    await fitForCapture(page);
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
      version: 4,
      graph: {
        nodes: [
          node("dev", { kind: "device", data: { device } }, { x: 0, y: 0, w: 420, h: 160 }),
          node("scope", { kind: "scope" }, { x: 540, y: 0, w: 1360, h: 360 }),
          channel("ch", "atv", { x: 540, y: 400, w: 460, h: 433 }),
          node("video", { kind: "video" }, { x: 1140, y: 400, w: 760, h: 596 }),
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
      return { params: { ...params, settings: { ...params.settings, interlace: true } } };
    });
    await fitForCapture(page);
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
      version: 4,
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
          slot("scope", { x: 0, y: 0, w: 8, h: 3 }),
          slot("map", { x: 8, y: 0, w: 4, h: 3 }),
          slot("log", { x: 0, y: 3, w: 8, h: 5 }),
          slot("readout", { x: 8, y: 3, w: 4, h: 5 }),
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
