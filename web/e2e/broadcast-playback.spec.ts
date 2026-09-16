import { expect, test } from "@playwright/test";
import type { StateSnapshot, WorkspaceSnapshot } from "../src/lib/types";

for (const system of ["dab", "dvbt", "dvbs", "dvbs2", "dvbs2x", "dvbs2sf"] as const) {
  test(`${system} plays recorded broadcast media`, async ({ page }) => {
    test.setTimeout(60_000);
    await page.addInitScript(() => {
      const scope = globalThis as unknown as { broadcastPcm: number };
      scope.broadcastPcm = 0;
      const Base = globalThis.AudioWorkletNode;
      globalThis.AudioWorkletNode = class extends Base {
        constructor(context: BaseAudioContext, name: string, options?: AudioWorkletNodeOptions) {
          super(context, name, options);
          const port = this.port as unknown as {
            postMessage: (message: unknown, transfer?: Transferable[]) => void;
          };
          const post = port.postMessage.bind(port);
          port.postMessage = (message, transfer) => {
            if (message instanceof Float32Array) scope.broadcastPcm += message.length;
            post(message, transfer);
          };
        }
      };
    });
    const type = system.startsWith("dvbs") ? "datv" : system;
    const sink = system === "dab" ? "readout" : "video";
    const inventory = await page.request.get("/api/devices").then((r) => r.json());
    const recording = inventory.devices.find((item: { key: string }) =>
      item.key.endsWith(`broadcast-${system}`),
    );
    expect(recording).toBeDefined();
    const key: string = recording.key;
    const snapshot: WorkspaceSnapshot = {
      version: 3,
      graph: {
        nodes: [
          {
            id: "radio",
            kind: "device",
            position: { x: 0, y: 0 },
            data: { device: { backend: "virtual", key } },
          },
          {
            id: "broadcast",
            kind: "channel",
            position: { x: 400, y: 0 },
            data: { channel_type: type },
          },
          { id: "speaker", kind: "speaker", position: { x: 800, y: 0 } },
          { id: "display", kind: sink, position: { x: 800, y: 300 } },
        ],
        edges: [
          { from: { node: "radio", port: "iq" }, to: { node: "broadcast", port: "iq" } },
          { from: { node: "broadcast", port: "audio" }, to: { node: "speaker", port: "audio" } },
          {
            from: { node: "broadcast", port: system === "dab" ? "events" : "video" },
            to: { node: "display", port: system === "dab" ? "events" : "video" },
          },
        ],
      },
    };
    const desk = await page.request.get("/api/workspaces").then((r) => r.json());
    const response = await page.request.post("/api/workspaces", {
      data: { name: `${system} playback`, snapshot },
    });
    expect(response.ok(), await response.text()).toBe(true);
    const { id } = await response.json();
    try {
      expect((await page.request.post(`/api/workspaces/${id}/activate`, { data: {} })).ok()).toBe(
        true,
      );
      expect((await page.request.post(`/api/workspaces/${id}/apply`, { data: {} })).ok()).toBe(
        true,
      );
      const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
      const device = state.device_sets.find((item) => item.device.key === key);
      expect(device).toBeDefined();
      const channel = device?.channels[0];
      expect(channel).toBeDefined();
      if (!device || !channel) throw new Error("Missing broadcast receiver");
      const params = channel.settings.params;
      const settings =
        params.type === "dab"
          ? { ...params.settings, mode: "dab_plus" }
          : params.type === "datv"
            ? {
                ...params.settings,
                standard: system.startsWith("dvbs2") ? "dvb_s2" : "dvb_s",
                symbol_rate: 250_000,
                superframes: system === "dvbs2sf",
                code_rate: "three_quarters",
              }
            : params.settings;
      const patched = await page.request.patch(
        `/api/devicesets/${device.id}/channels/${channel.id}`,
        {
          data: {
            ...channel.settings,
            frequency_hz: 220_352_000,
            squelch: { mode: "off" },
            params: { type: params.type, settings },
          },
        },
      );
      expect(patched.ok(), await patched.text()).toBe(true);
      await page.goto("/");
      const speaker = page.locator('.react-flow__node[data-id="speaker"]');
      await speaker.locator("header").click();
      await speaker.getByRole("button", { name: "Play", exact: true }).click();
      await expect
        .poll(
          () =>
            page.evaluate(() => (globalThis as unknown as { broadcastPcm: number }).broadcastPcm),
          { timeout: 30_000 },
        )
        .toBeGreaterThan(1000);
      const display = page.locator('.react-flow__node[data-id="display"]');
      await display.locator("header").click();
      if (system === "dab") {
        await expect(display.getByText("SDR-- live", { exact: true })).toBeVisible({
          timeout: 30_000,
        });
        await expect(display.getByRole("img")).toBeVisible();
        await expect(display.getByRole("link", { name: /Download/ })).toHaveAttribute(
          "download",
          /.+/,
        );
      } else {
        const canvas = display.getByLabel("Decoded video");
        await expect(canvas).toHaveAttribute("width", "160", { timeout: 30_000 });
        await expect(canvas).toHaveAttribute("height", "96");
        const range = await canvas.evaluate((element) => {
          const surface = element as HTMLCanvasElement;
          const bytes = surface.getContext("2d")?.getImageData(0, 0, 160, 96).data ?? [];
          let colored = 0;
          for (let i = 0; i < bytes.length; i += 4)
            if (Math.abs((bytes[i] ?? 0) - (bytes[i + 1] ?? 0)) > 50) colored++;
          return colored;
        });
        expect(range).toBeGreaterThan(100);
      }
    } finally {
      await page.request.post(`/api/workspaces/${desk.active}/activate`, { data: {} });
      await page.request.delete(`/api/workspaces/${id}`);
    }
  });
}
