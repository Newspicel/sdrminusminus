import { expect, test } from "@playwright/test";
import { formatMhz } from "../src/components/format";
import type { DecoderLogResponse, StateSnapshot, WorkspaceSnapshot } from "../src/lib/types";

test("monitors IQ through one node and exports transmission audio", async ({ page }) => {
  test.setTimeout(60000);
  const suffix = Date.now();
  const nodeIds = {
    radio: `radio-${suffix}`,
    monitor: `monitor-${suffix}`,
    log: `log-${suffix}`,
    export: `export-${suffix}`,
  };
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const previous = await page.request.get("/api/workspaces").then((response) => response.json());
  const snapshot: WorkspaceSnapshot = {
    version: 3,
    graph: {
      nodes: [
        {
          id: nodeIds.radio,
          kind: "device",
          position: { x: 0, y: 0 },
          data: { device: { backend: "virtual", key: "siggen" } },
        },
        {
          id: nodeIds.monitor,
          kind: "spectrum_monitor",
          position: { x: 380, y: 0 },
          data: { record_audio: true },
        },
        {
          id: nodeIds.log,
          kind: "decoder_log",
          position: { x: 760, y: 0 },
          size: { w: 620, h: 430 },
        },
        { id: nodeIds.export, kind: "export", position: { x: 380, y: 360 } },
      ],
      edges: [
        { from: { node: nodeIds.radio, port: "iq" }, to: { node: nodeIds.monitor, port: "iq" } },
        {
          from: { node: nodeIds.monitor, port: "events" },
          to: { node: nodeIds.log, port: "events" },
        },
        {
          from: { node: nodeIds.monitor, port: "events" },
          to: { node: nodeIds.export, port: "events" },
        },
      ],
    },
  };
  const created = await page.request.post("/api/workspaces", {
    data: { name: `Spectrum monitor test ${suffix}`, snapshot },
  });
  expect(created.ok()).toBe(true);
  const { id } = await created.json();
  expect((await page.request.post(`/api/workspaces/${id}/activate`, { data: {} })).ok()).toBe(true);
  try {
    await page.goto("/");
    const monitor = page.locator(`.react-flow__node[data-id="${nodeIds.monitor}"]`);
    const log = page.locator(`.react-flow__node[data-id="${nodeIds.log}"]`);
    await expect(monitor.locator(".react-flow__handle")).toHaveCount(2);
    await expect(monitor.locator("li")).toHaveCount(0);
    await expect(monitor).not.toContainText(/\d+ (active|recent)/);
    const confidence = monitor.getByRole("textbox", { name: "Minimum confidence (%)" });
    await expect(confidence).toHaveValue("70");
    await monitor.getByText("Spectrum monitor", { exact: true }).click();
    await confidence.click();
    await confidence.press("ControlOrMeta+A");
    await confidence.pressSequentially("80");
    await expect(confidence).toHaveValue("80");
    await confidence.press("Enter");
    await expect
      .poll(async () => {
        const workspace = await page.request
          .get(`/api/workspaces/${id}`)
          .then((response) => response.json());
        return workspace.snapshot.graph.nodes.find(
          (node: { id: string }) => node.id === nodeIds.monitor,
        ).data.min_confidence;
      })
      .toBeCloseTo(0.8);
    await page.reload();
    await expect(confidence).toHaveValue("80");
    await monitor.getByText("Spectrum monitor", { exact: true }).click();
    await confidence.click();
    await confidence.press("ControlOrMeta+A");
    await confidence.pressSequentially("0");
    await confidence.press("Enter");
    await expect(log).toContainText("Transmission", { timeout: 15000 });
    const state: StateSnapshot = await page.request
      .get("/api/state")
      .then((response) => response.json());
    const device = state.device_sets.find((candidate) => candidate.device.driver === "virtual");
    if (!device) throw new Error("virtual source missing");
    expect(device.channels).toHaveLength(0);
    expect(
      (
        await page.request.patch(`/api/devicesets/${device.id}/device`, {
          data: { center_hz: (device.settings.center_hz ?? 100000000) + 1000 },
        })
      ).ok(),
    ).toBe(true);
    let audioUrl: string | undefined;
    let audioSummary: string | undefined;
    let audioFrequency = 0;
    await expect
      .poll(
        async () => {
          const result: DecoderLogResponse = await page.request
            .get(`/api/decoderlog?sink=${nodeIds.log}&kind=transmission`)
            .then((response) => response.json());
          for (const row of result.entries) {
            if (row.event.kind === "transmission") {
              expect(row.event.data.state).not.toBe("started");
            }
            if (row.event.kind === "transmission" && row.event.data.audio) {
              expect(row.node).toBe(nodeIds.monitor);
              expect(row.origin?.node).toBe(nodeIds.monitor);
              audioUrl = row.event.data.audio.url;
              audioSummary = row.summary;
              audioFrequency = row.freq_hz;
            }
          }
          return audioUrl;
        },
        { timeout: 15000 },
      )
      .toBeTruthy();
    const audio = await page.request.get(audioUrl ?? "");
    expect(audio.ok()).toBe(true);
    const wav = await audio.body();
    expect(wav.subarray(0, 4).toString()).toBe("RIFF");
    expect(wav.length).toBeGreaterThan(44);
    const exported = await page.request.get(`/api/decoderlog/export/json?sink=${nodeIds.export}`);
    expect(exported.ok()).toBe(true);
    const exportedRows = await exported.json();
    expect(
      exportedRows.some(
        (entry: { origin?: { node: string } }) => entry.origin?.node === nodeIds.monitor,
      ),
    ).toBe(true);
    const row = log
      .locator("tr[aria-expanded]")
      .filter({ hasText: audioSummary })
      .filter({ hasText: formatMhz(audioFrequency) })
      .first();
    await expect(row).toBeVisible();
    await row.focus();
    await page.keyboard.press("Enter");
    await expect(row).toHaveAttribute("aria-expanded", "true");
    await expect(log.locator("audio")).toBeVisible();
    await page.screenshot({ path: "/tmp/spectrum-monitor.png", fullPage: true });
    expect(errors).toEqual([]);
  } finally {
    await page.request.post(`/api/workspaces/${previous.active}/activate`, { data: {} });
    await page.request.delete(`/api/workspaces/${id}`);
  }
});
