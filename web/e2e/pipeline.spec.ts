import { expect, test } from "@playwright/test";
import type { WorkspaceSnapshot } from "../src/lib/types";

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
    const created = await page.request.post("/api/workspaces", {
      data: { name: `Audio pipeline ${fallback}`, snapshot },
    });
    expect(created.ok()).toBe(true);
    const { id } = await created.json();
    expect((await page.request.post(`/api/workspaces/${id}/activate`, { data: {} })).ok()).toBe(
      true,
    );
    await page.goto("/");
    const speaker = page.locator('.react-flow__node[data-id="speaker"]');
    await speaker.locator("header").click();
    await speaker.getByRole("button", { name: "Play", exact: true }).click();
    await expect(speaker.getByRole("button", { name: "Stop", exact: true })).toBeVisible();
    await expect(speaker.getByText("Buffer", { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(
      page.locator('.react-flow__node[data-id="radio"]').getByText(/^Queue \d+ ms$/),
    ).toBeVisible();
    if (fallback) expect(workers.some((url) => url.includes("opusWorker"))).toBe(true);
    await speaker.getByRole("button", { name: "Stop", exact: true }).click();
    await expect(speaker.getByRole("button", { name: "Play", exact: true })).toBeVisible();
    expect(errors).toEqual([]);
  });
}
