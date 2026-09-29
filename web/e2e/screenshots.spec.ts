import { expect, test } from "@playwright/test";
import { listen, SCENES } from "./scenes";

const SHOTS = "../assets/screenshots";

for (const scene of SCENES) {
  test(scene.title, async ({ page }) => {
    await page.goto("/");
    await scene.stage(page);
    await scene.ready(page);
    if (scene.speaker !== undefined) {
      await listen(page, scene.speaker);
    }
    await page.waitForTimeout(scene.settleSeconds * 1000);
    const toasts = page.getByLabel("Dismiss", { exact: true });
    await expect(async () => {
      expect(await toasts.count()).toBe(0);
      await page.screenshot({ path: `${SHOTS}/${scene.id}.png` });
      expect(await toasts.count()).toBe(0);
    }).toPass({ timeout: 90_000 });
  });
}
