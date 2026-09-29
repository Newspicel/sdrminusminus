import { expect, test } from "@playwright/test";
import type { PatchNode, SurveyGrid, WorkspaceSnapshot } from "../src/lib/types";
import { face } from "./canvas";

function placed(id: string, body: Record<string, unknown>, x: number, y: number): PatchNode {
  return { id, position: { x, y }, size: { w: 420, h: 360 }, ...body } as PatchNode;
}

const SURVEY: WorkspaceSnapshot = {
  version: 4,
  graph: {
    nodes: [
      placed(
        "dev",
        { kind: "device", data: { device: { backend: "virtual", key: "siggen" } } },
        0,
        0,
      ),
      placed(
        "site",
        { kind: "gps", data: { source: { type: "fixed", lat: 52.52, lon: 13.405 } } },
        0,
        420,
      ),
      placed(
        "survey",
        { kind: "signal_map", data: { offset_hz: 0, bandwidth_hz: 12_500 } },
        480,
        0,
      ),
    ],
    edges: [
      { from: { node: "dev", port: "iq" }, to: { node: "survey", port: "iq" } },
      { from: { node: "site", port: "position" }, to: { node: "survey", port: "position" } },
    ],
  },
};

test("keeps surveyed cells on the server across a reload", async ({ page }) => {
  const response = await page.request.post("/api/workspaces", {
    data: { name: "Survey", snapshot: SURVEY },
  });
  const created: { id: number } = await response.json();
  await page.request.post(`/api/workspaces/${created.id}/activate`);
  expect((await page.request.post(`/api/workspaces/${created.id}/apply`)).ok()).toBe(true);

  await page.goto("/");
  const survey = face(page, "survey");
  await expect(survey.getByText("Ready")).toBeVisible({ timeout: 30_000 });
  await expect(survey.getByText(/^-?\d+\.\d dBFS$/)).toBeVisible();
  await survey.getByRole("button", { name: "Start survey" }).click();
  await expect(survey.getByText("1 cells")).toBeVisible({ timeout: 15_000 });
  await survey.getByRole("button", { name: "Pause" }).click();
  await expect(survey.getByRole("button", { name: "Start survey" })).toBeVisible();

  await page.reload();
  await expect(face(page, "survey").getByText("1 cells")).toBeVisible();
  const grid: SurveyGrid = await page.request
    .get("/api/survey/survey")
    .then((answer) => answer.json());
  expect(grid.cells).toHaveLength(1);
  expect(grid.cells[0]).toMatchObject({ latitude: 52.52, longitude: 13.405 });

  const cleared = await page.request.post("/api/survey/survey", { data: { action: "clear" } });
  expect(cleared.ok()).toBe(true);
  await expect(face(page, "survey").getByText("0 cells")).toBeVisible();
  await page.request.delete(`/api/workspaces/${created.id}`);
});
