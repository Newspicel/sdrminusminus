import { test } from "@playwright/test";
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
    await page.screenshot({ path: `${SHOTS}/${scene.id}.png` });
  });
}
