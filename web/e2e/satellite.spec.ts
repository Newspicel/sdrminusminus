import { type APIRequestContext, expect, type Page, test } from "@playwright/test";

let original: { id: string; snapshot: unknown } | undefined;

async function replaceGraph(request: APIRequestContext, graph: unknown) {
  const listed = await (await request.get("/api/workspaces")).json();
  const id = listed.active;
  const detail = await (await request.get(`/api/workspaces/${id}`)).json();
  original ??= { id, snapshot: detail.snapshot };
  const saved = await request.put(`/api/workspaces/${id}`, {
    data: { revision: detail.revision, snapshot: { ...detail.snapshot, graph } },
  });
  expect(saved.ok()).toBe(true);
}

test.afterEach(async ({ request }) => {
  if (!original) return;
  const { id, snapshot } = original;
  original = undefined;
  const detail = await (await request.get(`/api/workspaces/${id}`)).json();
  const restored = await request.put(`/api/workspaces/${id}`, {
    data: { revision: detail.revision, snapshot },
  });
  expect(restored.ok()).toBe(true);
});

test("a decoder wired to a satellite says who tunes it", async ({ page, request }) => {
  await page.route("https://tiles.openfreemap.org/**", (route) => route.abort());
  await page.goto("/");
  const graph = {
    nodes: [
      { id: "satellite:1", kind: "satellite", data: {}, position: { x: 0, y: 0 } },
      {
        id: "channel:1",
        kind: "channel",
        data: { channel_type: "nfm" },
        position: { x: 500, y: 0 },
      },
    ],
    edges: [
      {
        from: { node: "satellite:1", port: "control" },
        to: { node: "channel:1", port: "control" },
      },
    ],
  };
  await replaceGraph(request, graph);
  await page.reload();

  const decoder = page.locator('.react-flow__node[data-id="channel:1"]');
  const lock = decoder.getByRole("button", { name: /Tuned by Satellite/ });
  await expect(lock).toBeVisible();
  await lock.hover();
  await expect(
    page.getByText("Tuned by Satellite. Unwire its control to tune by hand."),
  ).toBeVisible();
  await lock.click({ force: true });
  await expect(lock).toBeVisible();
});

async function drawSatellite(page: Page, request: APIRequestContext, held: boolean) {
  await page.route("https://tiles.openfreemap.org/**", (route) => route.abort());
  await page.goto("/");
  const tle = [
    "ISS (ZARYA)",
    "1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999",
    "2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041",
  ].join("\n");
  const graph = {
    nodes: [
      {
        id: "satellite:1",
        kind: "satellite",
        data: { tle, downlink_hz: 145_800_000, transmitter: held ? "voice" : undefined },
        position: { x: 0, y: 0 },
      },
    ],
    edges: [],
  };
  await replaceGraph(request, graph);
  await page.reload();
  return page.locator('.react-flow__node[data-id="satellite:1"]');
}

test("a picked signal holds the downlink", async ({ page, request }) => {
  const satellite = await drawSatellite(page, request, true);
  const lock = satellite.getByRole("button", { name: /Set by the signal/ });
  await expect(lock).toBeVisible();
  await lock.hover();
  await expect(
    page.getByText("Set by the signal. Pick Own frequency to tune by hand."),
  ).toBeVisible();
  await expect(satellite.getByRole("button", { name: "Type the downlink" })).toBeDisabled();
});

test("an own downlink can be locked by hand", async ({ page, request }) => {
  const satellite = await drawSatellite(page, request, false);
  const typed = satellite.getByRole("button", { name: "Type the downlink" });
  await expect(typed).toBeEnabled();
  await satellite.getByRole("button", { name: "Lock tuning" }).click();
  await expect(typed).toBeDisabled();
  await satellite.getByRole("button", { name: "Unlock tuning" }).click();
  await expect(typed).toBeEnabled();
});
