import { expect, test } from "@playwright/test";
import type { StateSnapshot, WorkspaceDetail } from "../src/lib/types";

test("array composition preserves live Device faces and their channels", async ({ page }) => {
  const workspaces = await page.request.get("/api/workspaces").then((response) => response.json());
  const template: WorkspaceDetail = await page.request
    .get(`/api/workspaces/${workspaces.active}`)
    .then((response) => response.json());
  const snapshot = {
    ...template.snapshot,
    graph: {
      nodes: [
        {
          id: "left",
          kind: "device",
          position: { x: 0, y: 0 },
          data: { device: { backend: "virtual", key: "siggen" } },
        },
        {
          id: "right",
          kind: "device",
          position: { x: 350, y: 0 },
          data: { device: { backend: "virtual", key: "halfduplex" } },
        },
        {
          id: "pair",
          kind: "array",
          position: { x: 700, y: 0 },
          data: { members: 2, coherence: "time_sync", shared_tuning: true },
        },
        { id: "voice", kind: "channel", position: { x: 0, y: 600 }, data: { channel_type: "nfm" } },
      ],
      edges: [
        { from: { node: "left", port: "iq" }, to: { node: "pair", port: "iq" } },
        { from: { node: "right", port: "iq" }, to: { node: "pair", port: "iq2" } },
        { from: { node: "left", port: "iq" }, to: { node: "voice", port: "iq" } },
      ],
    },
    rack: { ...template.snapshot.rack, slots: [] },
  };
  const created = await page.request.post("/api/workspaces", {
    data: { name: "Array composition smoke", snapshot },
  });
  expect(created.ok()).toBe(true);
  const { id } = await created.json();
  expect((await page.request.post(`/api/workspaces/${id}/activate`, { data: {} })).ok()).toBe(true);
  await page.goto("/");
  for (const source of ["left", "right"]) {
    const face = page.locator(`.react-flow__node[data-id="${source}"]`);
    await expect(face.getByRole("button", { name: "Forget radio" })).toBeVisible();
    await expect(face.locator('[id^="frequency-dial"]')).toBeVisible();
    await expect(face.getByRole("button", { name: "Unlock tuning" })).toHaveCount(0);
    await expect(face.getByRole("combobox", { name: "Sample rate" })).toHaveCount(0);
  }
  const before: StateSnapshot = await page.request
    .get("/api/state")
    .then((response) => response.json());
  const source = before.device_sets.find((set) => set.device.key === "siggen");
  expect(before.device_sets).toHaveLength(3);
  expect(source?.channels).toHaveLength(1);
  const current: WorkspaceDetail = await page.request
    .get(`/api/workspaces/${id}`)
    .then((response) => response.json());
  expect(
    (
      await page.request.put(`/api/workspaces/${id}`, {
        data: {
          revision: current.revision,
          snapshot: {
            ...current.snapshot,
            graph: {
              nodes: current.snapshot.graph.nodes.filter((node) => node.id !== "pair"),
              edges: current.snapshot.graph.edges?.filter((edge) => edge.to.node !== "pair"),
            },
          },
        },
      })
    ).ok(),
  ).toBe(true);
  expect((await page.request.post(`/api/workspaces/${id}/apply`, { data: {} })).ok()).toBe(true);
  await expect
    .poll(async () => {
      const state: StateSnapshot = await page.request
        .get("/api/state")
        .then((response) => response.json());
      return state.device_sets.length;
    })
    .toBe(2);
  const after: StateSnapshot = await page.request
    .get("/api/state")
    .then((response) => response.json());
  expect(after.device_sets.find((set) => set.id === source?.id)?.channels).toHaveLength(1);
  await expect(
    page
      .locator('.react-flow__node[data-id="left"]')
      .getByRole("combobox", { name: "Sample rate" }),
  ).toBeVisible();
});
