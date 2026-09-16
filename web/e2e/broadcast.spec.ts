import { expect, test } from "@playwright/test";
import type { WorkspaceSnapshot } from "../src/lib/types";

test("persists all DAB transmission modes separately from audio generation", async ({ page }) => {
  const snapshot: WorkspaceSnapshot = {
    version: 3,
    graph: {
      nodes: [
        { id: "dab", kind: "channel", position: { x: 0, y: 0 }, data: { channel_type: "dab" } },
      ],
      edges: [],
    },
  };
  const desk = await page.request.get("/api/workspaces").then((response) => response.json());
  const created = await page.request.post("/api/workspaces", {
    data: { name: "DAB modes", snapshot },
  });
  expect(created.ok()).toBe(true);
  const { id } = await created.json();
  expect((await page.request.post(`/api/workspaces/${id}/activate`, { data: {} })).ok()).toBe(true);
  try {
    await page.goto("/");
    const node = page.locator('.react-flow__node[data-id="dab"]');
    await node.locator("header").click();
    const generation = node.getByRole("group", { name: "DAB generation", exact: true });
    await generation.getByRole("button", { name: "DAB+", exact: true }).click();
    const modes = node.getByRole("group", { name: "DAB transmission mode", exact: true });
    for (const mode of ["II", "III", "IV", "I"]) {
      await modes.getByRole("button", { name: mode, exact: true }).click();
      await expect(modes.getByRole("button", { name: mode, exact: true })).toHaveAttribute(
        "aria-pressed",
        "true",
      );
      await expect
        .poll(async () => {
          const detail = await page.request
            .get(`/api/workspaces/${id}`)
            .then((response) => response.json());
          return detail.state.channels.find((channel: { node: string }) => channel.node === "dab")
            ?.settings.params.settings;
        })
        .toMatchObject({ mode: "dab_plus", transmission_mode: mode.toLowerCase() });
    }
    await page.reload();
    await expect(modes.getByRole("button", { name: "I", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(generation.getByRole("button", { name: "DAB+", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  } finally {
    await page.request.post(`/api/workspaces/${desk.active}/activate`, { data: {} });
    await page.request.delete(`/api/workspaces/${id}`);
  }
});
