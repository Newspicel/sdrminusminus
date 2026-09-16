import { expect, type Page, test } from "@playwright/test";

async function captureOpens(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const opened: string[] = [];
    (window as unknown as { __opened: string[] }).__opened = opened;
    window.open = (url) => {
      opened.push(String(url));
      return null;
    };
  });
}

async function openedUrl(page: Page): Promise<URL> {
  const opened = await page.evaluate(() => (window as unknown as { __opened: string[] }).__opened);
  expect(opened).toHaveLength(1);
  return new URL(opened[0] ?? "");
}

async function openReport(page: Page) {
  await page
    .getByRole("button", { name: "Keyboard shortcuts, licenses and problem reports" })
    .click();
  await page.getByRole("button", { name: "Report a problem" }).click();
  return page.getByRole("dialog").filter({ hasText: "Report a problem" });
}

test("reporting a problem collects the diagnostics and prefills a GitHub issue", async ({
  page,
}) => {
  await captureOpens(page);
  await page.goto("/");
  const dialog = await openReport(page);
  await expect(dialog).toBeVisible();

  const bundle = dialog.locator("pre");
  await expect(bundle).toContainText("### Environment");
  await expect(bundle).toContainText("### Diagnostics");
  await expect(bundle).toContainText("| SDR-- |");

  await expect(bundle).not.toContainText("### Workspace");
  await dialog.getByRole("checkbox", { name: "Include the workspace shape" }).click();
  await expect(bundle).toContainText("### Workspace");

  await dialog.getByRole("textbox").first().fill("Device open failed");
  await dialog.getByRole("button", { name: "Open a GitHub issue" }).click();

  const url = await openedUrl(page);
  expect(url.hostname).toBe("github.com");
  expect(url.pathname).toMatch(/\/issues\/new$/);
  expect(url.searchParams.get("template")).toBe("bug.yml");
  expect(url.searchParams.get("title")).toBe("Device open failed");
  expect(url.searchParams.get("version")).toMatch(/^\d/);
  expect(url.href.length).toBeLessThanOrEqual(6000);
});

test("a feature request goes to its own form", async ({ page }) => {
  await captureOpens(page);
  await page.goto("/");
  const dialog = await openReport(page);
  await dialog.getByRole("button", { name: "Request a feature instead" }).click();

  const url = await openedUrl(page);
  expect(url.searchParams.get("template")).toBe("feature.yml");
});

test("the report survives a server that is not answering", async ({ page }) => {
  await captureOpens(page);
  await page.goto("/");
  await page.route("**/api/diagnostics", (route) => route.abort());

  const dialog = await openReport(page);
  await expect(dialog.getByText("The server did not answer")).toBeVisible();
  await expect(dialog.locator("pre")).toContainText("### Environment");
});
