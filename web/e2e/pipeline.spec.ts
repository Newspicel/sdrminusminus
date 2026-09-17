import { expect, type Page, test } from "@playwright/test";
import type { WorkletReport } from "../src/lib/audio/worklet";
import { decodeAudio } from "../src/lib/frame";
import type { ChannelSettings, StateSnapshot, WorkspaceSnapshot } from "../src/lib/types";

interface AudioProbe {
  frames: number;
  nonfinite: number;
  energy: number;
  lastPacket: number;
  longestGap: number;
  timeline: { frames: number; at: number; rendered: number; state: AudioContextState }[];
  reports: WorkletReport[];
  reportTimes: { at: number; rendered: number; frames: number }[];
}

type AudioScope = typeof globalThis & {
  audioProbe: AudioProbe;
  audioLoad: number;
  audioContext: AudioContext;
};

async function instrumentPlayback(page: Page, delayOutput: boolean): Promise<void> {
  await page.addInitScript((delayed) => {
    if (delayed) {
      const Context = globalThis.AudioContext;
      globalThis.AudioContext = class extends Context {
        constructor(options?: AudioContextOptions) {
          super(options);
          const suspended = this.suspend();
          const resume = super.resume.bind(this);
          this.resume = async () => {
            await suspended;
            await new Promise((resolve) => setTimeout(resolve, 750));
            await resume();
          };
          void this.resume();
        }
      };
    }
    const scope = globalThis as AudioScope;
    scope.audioProbe = {
      frames: 0,
      nonfinite: 0,
      energy: 0,
      lastPacket: 0,
      longestGap: 0,
      timeline: [],
      reports: [],
      reportTimes: [],
    };
    const Base = globalThis.AudioWorkletNode;
    globalThis.AudioWorkletNode = class extends Base {
      constructor(context: BaseAudioContext, name: string, options?: AudioWorkletNodeOptions) {
        super(context, name, options);
        scope.audioContext = context as AudioContext;
        this.port.addEventListener("message", (event: MessageEvent<WorkletReport>) => {
          scope.audioProbe.reports.push(event.data);
          scope.audioProbe.reportTimes.push({
            at: performance.now(),
            rendered: context.currentTime,
            frames: scope.audioProbe.frames,
          });
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
            probe.timeline.push({
              frames: probe.frames,
              at: now,
              rendered: context.currentTime,
              state: context.state,
            });
            for (const sample of message) {
              if (!Number.isFinite(sample)) probe.nonfinite++;
              else probe.energy += sample * sample;
            }
          }
          post(message, transfer);
        };
      }
    };
  }, delayOutput);
}

async function interruptPlayback(page: Page): Promise<void> {
  const speaker = page.locator('.react-flow__node[data-id="speaker"]');
  for (const duration of [750, 1500, 3000]) {
    await page.evaluate(() => (globalThis as AudioScope).audioContext.suspend());
    await expect(speaker.getByRole("button", { name: "Resume audio", exact: true })).toBeVisible();
    const paused = await page.evaluate(async (milliseconds) => {
      const probe = (globalThis as AudioScope).audioProbe;
      const before = probe.frames;
      await new Promise((resolve) => setTimeout(resolve, milliseconds));
      return { before, after: probe.frames };
    }, duration);
    expect(paused.after).toBe(paused.before);
    await page.evaluate(() => {
      (globalThis as AudioScope).audioProbe.lastPacket = 0;
    });
    await speaker.getByRole("button", { name: "Resume audio", exact: true }).click();
    await expect
      .poll(() => page.evaluate(() => (globalThis as AudioScope).audioProbe.frames))
      .toBeGreaterThan(paused.after + 96_000);
    const probe = await page.evaluate(() => (globalThis as AudioScope).audioProbe);
    await test.info().attach(`audio-recovery-${duration}`, {
      body: JSON.stringify(probe),
      contentType: "application/json",
    });
    expect(probe.nonfinite).toBe(0);
    expect(probe.longestGap).toBeLessThan(150);
    expect(Math.max(...probe.reports.map((report) => report.underruns))).toBe(0);
    expect(Math.max(...probe.reports.map((report) => report.trimmedFrames ?? 0))).toBe(0);
  }
  await page.evaluate(() => (globalThis as AudioScope).audioContext.suspend());
  await expect(speaker.getByRole("button", { name: "Resume audio", exact: true })).toBeVisible();
  await speaker.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(speaker.getByRole("button", { name: "Play", exact: true })).toBeVisible();
  const stopped = await page.evaluate(async () => {
    const probe = (globalThis as AudioScope).audioProbe;
    const before = probe.frames;
    await new Promise((resolve) => setTimeout(resolve, 750));
    return { before, after: probe.frames };
  });
  expect(stopped.after).toBe(stopped.before);
}

for (const { fallback, delayOutput, wideband } of [
  { fallback: false, delayOutput: false, wideband: false },
  { fallback: true, delayOutput: false, wideband: false },
  { fallback: false, delayOutput: true, wideband: false },
  { fallback: true, delayOutput: true, wideband: false },
  { fallback: false, delayOutput: false, wideband: true },
  { fallback: true, delayOutput: false, wideband: true },
]) {
  test(`plays virtual radio audio with ${fallback ? "WASM worker" : "native decoder"}${delayOutput ? " and delayed output" : ""}${wideband ? " and stereo changes" : ""}`, async ({
    page,
  }) => {
    test.setTimeout(60_000);
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const workers: string[] = [];
    page.on("worker", (worker) => workers.push(worker.url()));
    const layouts: number[] = [];
    page.on("websocket", (socket) => {
      socket.on("framereceived", ({ payload }) => {
        if (typeof payload === "string") return;
        const audio = decodeAudio(Uint8Array.from(payload).buffer);
        if (audio && layouts.at(-1) !== audio.chLayout) layouts.push(audio.chLayout);
      });
    });
    if (fallback) {
      await page.addInitScript(() =>
        Object.defineProperty(globalThis, "AudioDecoder", { value: undefined, configurable: true }),
      );
    }
    await instrumentPlayback(page, delayOutput);
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
            data: { channel_type: wideband ? "wfm" : "nfm" },
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
              data: {
                ...settings,
                frequency_hz: settings.frequency_hz + offset,
                params:
                  settings.params.type === "wfm"
                    ? {
                        ...settings.params,
                        settings: { ...settings.params.settings, stereo: offset === -100 },
                      }
                    : settings.params,
              } satisfies ChannelSettings,
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
      expect(probe.timeline.every((packet) => packet.state === "running")).toBe(true);
      expect(probe.energy).toBeGreaterThan(1);
      expect(probe.longestGap).toBeLessThan(150);
      expect(probe.reports.length).toBeGreaterThanOrEqual(12);
      expect(Math.max(...probe.reports.map((report) => report.underruns))).toBe(0);
      expect(Math.max(...probe.reports.map((report) => report.trimmedFrames ?? 0))).toBe(0);
      await expect(
        page.locator('.react-flow__node[data-id="radio"]').getByRole("status"),
      ).toHaveText("1/1");
      expect(workers.filter((url) => url.includes("opusWorker"))).toHaveLength(fallback ? 1 : 0);
      expect(layouts).toEqual(wideband ? [2, 1, 2, 1] : [1]);
      if (wideband) await speaker.getByRole("button", { name: "Stop", exact: true }).click();
      else await interruptPlayback(page);
      await expect(speaker.getByRole("button", { name: "Play", exact: true })).toBeVisible();
      const beforeRestart = await page.evaluate(() => {
        const restartingProbe = (globalThis as AudioScope).audioProbe;
        restartingProbe.lastPacket = 0;
        return restartingProbe.frames;
      });
      await speaker.getByRole("button", { name: "Play", exact: true }).click();
      await expect
        .poll(() => page.evaluate(() => (globalThis as AudioScope).audioProbe.frames))
        .toBeGreaterThan(beforeRestart + 48_000);
      await speaker.getByRole("button", { name: "Stop", exact: true }).click();
      expect(errors).toEqual([]);
    } finally {
      await page.request.post(`/api/workspaces/${desk.active}/activate`);
      await page.request.delete(`/api/workspaces/${id}`);
    }
  });
}
