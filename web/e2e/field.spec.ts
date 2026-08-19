import { expect, type Page, test } from "@playwright/test";
import type { WorkspaceDetail, WorkspaceSnapshot, WorkspacesResponse } from "../src/lib/types";

const LANES = 4;

async function activeWorkspace(page: Page): Promise<WorkspaceDetail> {
  const list: WorkspacesResponse = await page.request
    .get("/api/workspaces")
    .then((response) => response.json());
  return page.request
    .get(`/api/workspaces/${list.active}`)
    .then((response) => response.json() as Promise<WorkspaceDetail>);
}

/// Puts a four-lane virtual array and a direction finder into the active workspace, then applies
/// the patch, which is what makes the field client's DF drive mission have something to show.
async function stageArray(page: Page): Promise<void> {
  const detail = await activeWorkspace(page);
  const snapshot: WorkspaceSnapshot = structuredClone(detail.snapshot);
  const device = snapshot.graph.nodes.find((node) => node.kind === "device");
  if (device === undefined || device.kind !== "device") {
    throw new Error("the starter workspace opens with a receiver");
  }
  device.data.device = { backend: "virtual", key: "array4" };
  snapshot.graph.nodes.push({
    id: "df",
    kind: "df",
    data: {
      settings: {
        geometry: { kind: "uca", radius_m: 0.35, count: LANES },
        algorithm: "music",
        report_ms: 200,
        offset_hz: 25_000,
        bandwidth_hz: 20_000,
        sources: 1,
        beam_bearing_deg: null,
        station_id: null,
        cal: { source: "signal", bandwidth_hz: 200_000, pilot_hz: null, track: true },
      },
    },
    position: { x: 700, y: 320 },
  });
  for (let lane = 0; lane < LANES; lane++) {
    const port = lane === 0 ? "iq" : `iq${lane + 1}`;
    snapshot.graph.edges = [
      ...(snapshot.graph.edges ?? []),
      { from: { node: device.id, port }, to: { node: "df", port } },
    ];
  }
  const put = await page.request.put(`/api/workspaces/${detail.id}`, {
    data: { revision: detail.revision, snapshot },
  });
  expect(put.ok(), await put.text()).toBeTruthy();
  const applied = await page.request.post(`/api/workspaces/${detail.id}/apply`, { data: {} });
  expect(applied.ok(), await applied.text()).toBeTruthy();

  const device_settings = await page.request.get("/api/state").then((r) => r.json());
  const set = device_settings.device_sets[0];
  const patched = await page.request.patch(`/api/devicesets/${set.id}/device`, {
    data: {
      center_hz: 300_000_000,
      sample_rate: 1_024_000,
      extra: [
        { name: "wavefront_bearing_deg", value: 137 },
        { name: "array_radius_m", value: 0.35 },
      ],
    },
  });
  expect(patched.ok(), await patched.text()).toBeTruthy();
}

test.describe("field mode", () => {
  test("offers the missions the workspace can drive", async ({ page }) => {
    await page.goto("/");
    await stageArray(page);
    await page.goto("/field");
    await expect(page.getByRole("button", { name: /DF drive/ })).toBeVisible();
  });

  test("drives a direction finder from a phone", async ({ page }) => {
    await page.goto("/");
    await stageArray(page);
    await page.goto("/field");
    await page.getByRole("button", { name: /DF drive/ }).click();
    await expect(page.getByRole("img", { name: "Bearing relative to the vehicle" })).toBeVisible();
    await expect(page.getByText(/^\d{3}°$/)).toBeVisible({ timeout: 30_000 });
    await page.getByRole("button", { name: "← Missions" }).click();
    await expect(page.getByRole("button", { name: /DF drive/ })).toBeVisible();
  });
});
