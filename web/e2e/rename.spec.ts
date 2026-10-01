import { expect, type Locator, type Page, test } from "@playwright/test";
import type { WorkspaceDetail, WorkspacesResponse } from "../src/lib/types";
import { face, fitPatch, node, stage, unstage } from "./canvas";

async function label(page: Page): Promise<string | null | undefined> {
  const listed: WorkspacesResponse = await page.request
    .get("/api/workspaces")
    .then((r) => r.json());
  const detail: WorkspaceDetail = await page.request
    .get(`/api/workspaces/${listed.active}`)
    .then((r) => r.json());
  return detail.snapshot.graph.nodes.find((item) => item.id === "speaker")?.label;
}

function title(shell: Locator, name: string): Locator {
  return shell.getByRole("button", { name: `Rename ${name}`, exact: true });
}

function field(shell: Locator): Locator {
  return shell.getByRole("textbox", { name: "Node name", exact: true });
}

test.beforeEach(async ({ page }) => {
  await stage(page, "Node names", {
    version: 4,
    graph: {
      nodes: [node("speaker", { kind: "speaker" }, { x: 0, y: 0, w: 280, h: 180 })],
      edges: [],
    },
  });
  await fitPatch(page);
});

test.afterEach(({ request }) => unstage(request));

test("renames, survives a reload and undoes", async ({ page }) => {
  const speaker = face(page, "speaker");
  await title(speaker, "Speaker").dblclick();
  await expect(field(speaker)).toBeFocused();
  await field(speaker).fill("  Desk audio  ");
  await field(speaker).press("Enter");
  await expect(title(speaker, "Desk audio")).toBeVisible();
  await expect.poll(() => label(page)).toBe("Desk audio");

  await page.reload();
  await expect(title(speaker, "Desk audio")).toBeVisible();
  await page.getByRole("button", { name: /^undo/i }).click();
  await expect(title(speaker, "Speaker")).toBeVisible();
  await expect.poll(() => label(page)).toBeUndefined();
});

test("Escape keeps the old name and a blank name restores the default", async ({ page }) => {
  const speaker = face(page, "speaker");
  await title(speaker, "Speaker").dblclick();
  await field(speaker).fill("Room audio");
  await field(speaker).press("Escape");
  await expect(title(speaker, "Speaker")).toBeVisible();

  await title(speaker, "Speaker").focus();
  await page.keyboard.press("F2");
  await field(speaker).fill("Room audio");
  await field(speaker).blur();
  await expect.poll(() => label(page)).toBe("Room audio");

  await title(speaker, "Room audio").dblclick();
  await field(speaker).press("Backspace");
  await field(speaker).press("Enter");
  await expect(speaker).toHaveCount(1);
  await expect(title(speaker, "Speaker")).toBeVisible();
  await expect.poll(() => label(page)).toBeUndefined();
});
