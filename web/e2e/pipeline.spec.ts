import { expect, test } from "@playwright/test";
import type { WorkletReport } from "../src/lib/audio/worklet";
import type { StateSnapshot, WorkspaceSnapshot } from "../src/lib/types";

interface AudioProbe {
  frames: number;
  nonfinite: number;
  energy: number;
  lastPacket: number;
  longestGap: number;
  startup: { frames: number; at: number; rendered: number; state: AudioContextState }[];
  reports: WorkletReport[];
}

type AudioScope = typeof globalThis & { audioProbe: AudioProbe; audioLoad: number };

for (const fallback of [false, true]) {
  test(`plays virtual radio audio with ${fallback ? "WASM worker" : "native decoder"}`, async ({
    page,
  }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const workers: string[] = [];
    page.on("worker", (worker) => workers.push(worker.url()));
    if (fallback) {
      await page.addInitScript(() =>
        Object.defineProperty(globalThis, "AudioDecoder", { value: undefined, configurable: true }),
      );
    }
    await page.addInitScript(() => {
      const scope = globalThis as AudioScope;
      scope.audioProbe = {
        frames: 0,
        nonfinite: 0,
        energy: 0,
        lastPacket: 0,
        longestGap: 0,
        startup: [],
        reports: [],
      };
      const Base = globalThis.AudioWorkletNode;
      globalThis.AudioWorkletNode = class extends Base {
        constructor(context: BaseAudioContext, name: string, options?: AudioWorkletNodeOptions) {
          super(context, name, options);
          this.port.addEventListener("message", (event: MessageEvent<WorkletReport>) => {
            scope.audioProbe.reports.push(event.data);
          });
          const port = this.port as unknown as {
            postMessage: (message: unknown, transfer?: Transferable[]) => void;
          };
          const post = port.postMessage.bind(port);
          port.postMessage = (message, transfer) => {
            if (message instanceof Float32Array) {
              const probe = scope.audioProbe;
              const now = performance.now();
              if (probe.lastPacket > 0) {
                probe.longestGap = Math.max(probe.longestGap, now - probe.lastPacket);
              }
              probe.lastPacket = now;
              probe.frames += message.length / 2;
              if (probe.startup.length < 100) {
                probe.startup.push({
                  frames: probe.frames,
                  at: now,
                  rendered: context.currentTime,
                  state: context.state,
                });
              }
              for (const sample of message) {
                if (!Number.isFinite(sample)) probe.nonfinite++;
                else probe.energy += sample * sample;
              }
            }
            post(message, transfer);
          };
        }
      };
    });
    const snapshot: WorkspaceSnapshot = {
      version: 3,
      graph: {
        nodes: [
          {
            id: "radio",
            kind: "device",
            position: { x: 0, y: 0 },
            data: { device: { backend: "virtual", key: "siggen" } },
          },
          {
            id: "voice",
            kind: "channel",
            position: { x: 350, y: 0 },
            data: { channel_type: "nfm" },
          },
          { id: "speaker", kind: "speaker", position: { x: 700, y: 0 } },
        ],
        edges: [
          { from: { node: "radio", port: "iq" }, to: { node: "voice", port: "iq" } },
          { from: { node: "voice", port: "audio" }, to: { node: "speaker", port: "audio" } },
        ],
      },
    };
    const desk = await page.request.get("/api/workspaces").then((response) => response.json());
    const created = await page.request.post("/api/workspaces", {
      data: { name: `Audio pipeline ${fallback}`, snapshot },
    });
    expect(created.ok()).toBe(true);
    const { id } = await created.json();
    expect((await page.request.post(`/api/workspaces/${id}/activate`, { data: {} })).ok()).toBe(
      true,
    );
    try {
      await page.goto("/");
      await expect(
        page.locator('.react-flow__node[data-id="radio"]').getByRole("status"),
      ).toHaveText("1/1");
      const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
      const device = state.device_sets[0];
      const channel = device?.channels[0];
      if (!device || !channel) throw new Error("virtual channel was not created");
      const channelUrl = `/api/devicesets/${device.id}/channels/${channel.id}`;
      const settings = {
        ...channel.settings,
        frequency_hz: (device.settings.center_hz ?? 100_000_000) + 300_000,
        squelch: { mode: "off" as const },
      };
      expect((await page.request.patch(channelUrl, { data: settings })).ok()).toBe(true);
      const speaker = page.locator('.react-flow__node[data-id="speaker"]');
      await speaker.locator("header").click();
      await speaker.getByRole("button", { name: "Play", exact: true }).click();
      await expect(speaker.getByRole("button", { name: "Stop", exact: true })).toBeVisible();
      await expect
        .poll(() => page.evaluate(() => (globalThis as AudioScope).audioProbe.frames), {
          timeout: 15_000,
        })
        .toBeGreaterThan(48_000);
      await page.evaluate(() => {
        const scope = globalThis as AudioScope;
        scope.audioLoad = window.setInterval(() => {
          const until = performance.now() + 12;
          while (performance.now() < until) Math.sqrt(Math.random());
        }, 50);
      });
      for (const offset of [100, -100, 0]) {
        expect(
          (
            await page.request.patch(channelUrl, {
              data: { ...settings, frequency_hz: settings.frequency_hz + offset },
            })
          ).ok(),
        ).toBe(true);
        const frames = await page.evaluate(() => (globalThis as AudioScope).audioProbe.frames);
        await expect
          .poll(() => page.evaluate(() => (globalThis as AudioScope).audioProbe.frames))
          .toBeGreaterThan(frames + 96_000);
      }
      const probe = await page.evaluate(() => {
        const scope = globalThis as AudioScope;
        clearInterval(scope.audioLoad);
        return scope.audioProbe;
      });
      await test.info().attach("audio-probe", {
        body: JSON.stringify(probe),
        contentType: "application/json",
      });
      expect(probe.nonfinite).toBe(0);
      expect(probe.energy).toBeGreaterThan(1);
      expect(probe.longestGap).toBeLessThan(150);
      expect(probe.reports.length).toBeGreaterThanOrEqual(12);
      expect(Math.max(...probe.reports.map((report) => report.underruns))).toBe(0);
      expect(Math.max(...probe.reports.map((report) => report.trimmedFrames ?? 0))).toBe(0);
      await expect(
        page.locator('.react-flow__node[data-id="radio"]').getByRole("status"),
      ).toHaveText("1/1");
      expect(workers.some((url) => url.includes("opusWorker"))).toBe(fallback);
      await speaker.getByRole("button", { name: "Stop", exact: true }).click();
      await expect(speaker.getByRole("button", { name: "Play", exact: true })).toBeVisible();
      await page.evaluate(() => {
        (globalThis as AudioScope).audioProbe.lastPacket = 0;
      });
      await speaker.getByRole("button", { name: "Play", exact: true }).click();
      await expect
        .poll(() => page.evaluate(() => (globalThis as AudioScope).audioProbe.frames))
        .toBeGreaterThan(probe.frames + 48_000);
      await speaker.getByRole("button", { name: "Stop", exact: true }).click();
      expect(errors).toEqual([]);
    } finally {
      await page.request.post(`/api/workspaces/${desk.active}/activate`);
      await page.request.delete(`/api/workspaces/${id}`);
    }
  });
}
