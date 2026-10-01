import { test as base, expect, type Page } from "@playwright/test";
import { serverURL } from "../playwright.screenshots.config";
import { listen, SCENES } from "./scenes";

const SHOTS = "../assets/screenshots";
const REPAINT_MS = 2_000;

const test = base.extend({
  baseURL: async ({ baseURL }, use, testInfo) => {
    await use(serverURL(baseURL ?? "", testInfo.parallelIndex));
  },
});

async function capture(page: Page, path: string): Promise<void> {
  const toasts = page.getByLabel("Dismiss", { exact: true });
  await expect(async () => {
    expect(await toasts.count()).toBe(0);
    await page.screenshot({ path });
    expect(await toasts.count()).toBe(0);
  }).toPass({ timeout: 90_000 });
}

for (const scene of SCENES) {
  test(scene.title, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    await page.goto("/");
    await scene.stage(page);
    await scene.ready(page);
    if (scene.speaker !== undefined) {
      await listen(page, scene.speaker);
    }
    await page.waitForTimeout(scene.settleSeconds * 1000);
    await capture(page, `${SHOTS}/${scene.id}-dark.png`);
    await page.emulateMedia({ colorScheme: "light" });
    await page.waitForTimeout(REPAINT_MS);
    await capture(page, `${SHOTS}/${scene.id}-light.png`);
  });
}
