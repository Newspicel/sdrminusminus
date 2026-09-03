import { expect, type Page, test } from "@playwright/test";
import type { PatchNode, WorkspaceSnapshot } from "../src/lib/types";

const LANES = 4;

function arrayPatch(): WorkspaceSnapshot {
  const nodes = [
    {
      id: "dev",
      kind: "device",
      data: { device: { backend: "virtual", key: "array4" } },
      position: { x: 0, y: 0 },
      size: { w: 380, h: 420 },
    },
    {
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
    },
  ] as PatchNode[];
  return {
    version: 3,
    graph: {
      nodes,
      edges: Array.from({ length: LANES }, (_, lane) => {
        const port = lane === 0 ? "iq" : `iq${lane + 1}`;
        return { from: { node: "dev", port }, to: { node: "df", port } };
      }),
    },
  };
}

/// Stages a four-lane virtual array and a direction finder, which is what makes the field client's
/// DF drive mission have something to show. In a workspace of its own: every spec here shares one
/// server, so staging into the active one hands the next test whatever the last one left behind.
async function stageArray(page: Page, name: string): Promise<void> {
  const response = await page.request.post("/api/workspaces", {
    data: { name, snapshot: arrayPatch() },
  });
  const created: { id?: number; error?: string } = await response.json();
  if (created.id === undefined) {
    throw new Error(`workspace ${name} was rejected: ${created.error ?? response.status()}`);
  }
  await page.request.post(`/api/workspaces/${created.id}/activate`);
  const applied = await page.request.post(`/api/workspaces/${created.id}/apply`, { data: {} });
  expect(applied.ok(), await applied.text()).toBeTruthy();

  const state = await page.request.get("/api/state").then((r) => r.json());
  const set = state.device_sets[0];
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
    await stageArray(page, "Field missions");
    await page.goto("/field");
    await expect(page.getByRole("button", { name: /DF drive/ })).toBeVisible();
    await expect(page.getByRole("button", { name: /Fox hunt/ })).toHaveCount(0);
  });

  test("drives a direction finder from a phone", async ({ page }) => {
    await page.goto("/");
    await stageArray(page, "Field DF drive");
    await page.goto("/field");
    await page.getByRole("button", { name: /DF drive/ }).click();
    await expect(page.getByRole("img", { name: "Bearing relative to the vehicle" })).toBeVisible();
    await expect(page.getByText(/^\d{3}°$/)).toBeVisible({ timeout: 30_000 });
    await page.getByRole("button", { name: "← Missions" }).click();
    await expect(page.getByRole("button", { name: /DF drive/ })).toBeVisible();
  });
});
