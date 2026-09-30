import { type APIRequestContext, expect, type Locator, type Page, test } from "@playwright/test";
import { WS_BEARER_PROTOCOL_PREFIX, WS_SUBPROTOCOL } from "../src/generated/frame";
import { hexUtf8 } from "../src/lib/hex";
import type { AboutResponse, PairingOffer, PairResponse } from "../src/lib/types";
import { addNode, stage, unstage } from "./canvas";

test.describe.configure({ mode: "serial" });
test.use({ ignoreHTTPSErrors: true });
test.afterEach(({ request }) => unstage(request));

const CODE_TITLE = "Type this on the phone if the camera fails";

function phonePort(baseURL: string | undefined): number {
  if (baseURL === undefined) {
    throw new Error("a base URL to derive the phone port from");
  }
  return Number(new URL(baseURL).port) + 1;
}

async function protocol(request: APIRequestContext): Promise<number> {
  const about: AboutResponse = await request.get("/api/about").then((response) => response.json());
  return about.protocol;
}

async function pair(
  request: APIRequestContext,
  port: number,
  code: string,
  name: string,
): Promise<PairResponse> {
  const response = await request.post(`https://127.0.0.1:${port}/api/phones/pair`, {
    data: { code, name, platform: "android", protocol: await protocol(request) },
  });
  expect(response.status()).toBe(200);
  return response.json();
}

async function openPhones(page: Page): Promise<Locator> {
  const tab = page.getByRole("tab", { name: "Phones" });
  if (!(await tab.isVisible())) {
    await page.getByRole("button", { name: "Library" }).click();
  }
  await tab.click();
  return page.getByRole("tabpanel", { name: "Phones" });
}

async function closePhones(page: Page): Promise<void> {
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tab", { name: "Phones" })).toBeHidden();
}

async function phonesOff(request: APIRequestContext, port: number): Promise<void> {
  const response = await request.put("/api/phones/access", { data: { enabled: false, port } });
  expect(response.ok()).toBe(true);
}

async function addGps(page: Page): Promise<Locator> {
  await addNode(page, "GPS position");
  const added = page.locator('.react-flow__node[data-id^="gps:"]').last();
  await expect(added).toBeVisible();
  const id = await added.getAttribute("data-id");
  const node = page.locator(`.react-flow__node[data-id="${id}"]`);
  await node.locator("header").click();
  return node;
}

async function phoneSocket(page: Page, port: number, token: string): Promise<void> {
  await page.goto(`https://127.0.0.1:${port}/api/about`);
  await page.evaluate(
    async ({ url, protocols }) => {
      const socket = new WebSocket(url, protocols);
      await new Promise<void>((resolve, reject) => {
        socket.onmessage = (event: MessageEvent<string>) => {
          if ((JSON.parse(event.data) as { type: string }).type === "Hello") {
            resolve();
          }
        };
        socket.onclose = (event) => reject(new Error(`the phone socket closed with ${event.code}`));
      });
      const publish = (): void =>
        socket.send(
          JSON.stringify({
            type: "PublishPose",
            data: {
              fix: {
                latitude: 52.52,
                longitude: 13.405,
                heading_deg: 91,
                heading_source: "compass",
                time: new Date().toISOString(),
              },
            },
          }),
        );
      publish();
      const timer = setInterval(publish, 500);
      Object.assign(window, {
        hangUp: () => {
          clearInterval(timer);
          socket.close();
        },
      });
    },
    {
      url: `wss://127.0.0.1:${port}/api/ws`,
      protocols: [WS_SUBPROTOCOL, WS_BEARER_PROTOCOL_PREFIX + hexUtf8(token)],
    },
  );
}

test("pairs a phone from the library", async ({ page, baseURL }) => {
  const port = phonePort(baseURL);
  await page.goto("/");
  const panel = await openPhones(page);
  await expect(panel.getByText("No phones")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Pair phone" })).toBeDisabled();

  const portField = panel.getByRole("textbox", { name: "Port" });
  await portField.fill(String(port));
  const saved = page.waitForResponse("**/api/phones/access");
  await portField.press("Enter");
  await saved;
  await panel.getByRole("switch", { name: "Allow phones" }).click();
  await expect(panel.getByText(`Ready on ${port}`)).toBeVisible();

  await panel.getByRole("button", { name: "Pair phone" }).click();
  await expect(panel.getByRole("img", { name: "Pairing QR code" })).toBeVisible();
  const grouped = await panel.getByTitle(CODE_TITLE).textContent();
  expect(grouped).toMatch(/^\d{4} \d{4}$/);
  await expect(panel.getByTitle("Check this on the phone")).toHaveText(
    /^[0-9A-F]{4}( [0-9A-F]{4}){4}$/,
  );

  await pair(page.request, port, (grouped ?? "").replace(" ", ""), "E2E phone");
  await expect(panel.getByRole("button", { name: "E2E phone", exact: true })).toBeVisible();
  await expect(panel.getByRole("img", { name: "Pairing QR code" })).toBeHidden();
  await expect(page.getByText("Paired E2E phone")).toBeVisible();

  await panel.getByRole("button", { name: "Revoke E2E phone" }).click();
  await panel.getByRole("button", { name: "Revoke?" }).click();
  await expect(panel.getByText("No phones")).toBeVisible();
  await phonesOff(page.request, port);
});

test("phone GPS source shows heading", async ({ page, context, baseURL }) => {
  const port = phonePort(baseURL);
  const opened = await page.request.put("/api/phones/access", { data: { enabled: true, port } });
  expect(opened.ok()).toBe(true);
  const offer: PairingOffer = await page.request
    .post("/api/phones/offers", { data: {} })
    .then((response) => response.json());
  const paired = await pair(page.request, port, offer.code, "Heading phone");

  await stage(page, "Phone GPS", { version: 4, graph: { nodes: [], edges: [] } });
  const gps = await addGps(page);
  await gps
    .getByRole("group", { name: "Position source" })
    .getByText("Phone", { exact: true })
    .click();
  await gps.getByRole("button", { name: /Heading phone/ }).click();
  await expect(gps.getByRole("combobox", { name: "Phone" })).toHaveText("Heading phone");
  await expect(gps.getByText("Offline", { exact: true })).toBeVisible();

  const phone = await context.newPage();
  await phoneSocket(phone, port, paired.token);
  await expect(gps.getByText("52.520000, 13.405000")).toBeVisible();
  await expect(gps.getByText("091° compass")).toBeVisible();
  await expect(gps.getByText("Online", { exact: true })).toBeVisible();

  const panel = await openPhones(page);
  await expect(panel.getByText("Online", { exact: true })).toBeVisible();
  await closePhones(page);

  await phone.evaluate(() => (window as unknown as { hangUp: () => void }).hangUp());
  await expect(gps.getByText("Offline", { exact: true })).toBeVisible();

  const revoked = await page.request.delete(`/api/phones/${paired.phone.id}`);
  expect(revoked.status()).toBe(204);
  await expect(gps.getByText("Not paired", { exact: true })).toBeVisible();
  await phonesOff(page.request, port);
});
