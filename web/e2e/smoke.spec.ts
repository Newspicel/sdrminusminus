// The station, end to end: open the app, bind the virtual radio to the receiver node, add a
// channel, hear the graph become a running station, pin a face and find it on the rack.
//
// This is the one test that exercises the composition — canvas, faces, WebSocket, apply — that
// the unit suite cannot reach (PLAN §14). It asserts behaviour, not markup: what an operator
// would check after each gesture.
import { expect, type Locator, type Page, test } from "@playwright/test";

/** Draw a wire between two ports the way a pointer does. */
async function dragWire(page: Page, from: Locator, to: Locator): Promise<void> {
  const start = await from.boundingBox();
  const end = await to.boundingBox();
  if (start === null || end === null) {
    throw new Error("a port to wire from and one to wire to");
  }
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(end.x + end.width / 2, end.y + end.height / 2, { steps: 12 });
  await page.mouse.up();
}

test.describe("the station", () => {
  test("binds a radio, adds a channel and pins a face", async ({ page }) => {
    await page.goto("/");

    // The default station: a receiver node with nothing in it, a scope and a speaker. Nodes are
    // addressed by the id the stored patch gives them, which is the identity the server owns.
    const node = (id: string) => page.locator(`.react-flow__node[data-id="${id}"]`);
    const receiver = node("device");
    await expect(receiver).toBeVisible();
    await expect(node("scope")).toBeVisible();
    await expect(node("speaker")).toBeVisible();

    // Binding the virtual radio is the first gesture: the node *is* the "open a radio" prompt.
    await receiver.getByRole("button", { name: /signal generator/i }).click();
    // The radio is open and the node became the instrument: its dial is the signature element.
    await expect(receiver.locator('[id^="frequency-dial"]')).toBeVisible();

    // A channel is added as a node, and the server's apply creates the engine channel behind it.
    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "NFM", exact: true }).click();
    await expect(page.locator('.react-flow__node[data-id^="channel:"]')).toBeVisible();

    // Wiring the receiver to it is what makes it a channel on that radio, and apply creates it.
    // The drag needs intermediate moves: React Flow starts a connection on pointer *movement*
    // after the press, so a straight down-then-up never becomes a wire.
    await dragWire(
      page,
      receiver.locator('.react-flow__handle[data-handleid="iq"]'),
      page.locator(
        '.react-flow__node[data-id^="channel:"] .react-flow__handle[data-handleid="iq"]',
      ),
    );

    // The state the server reports is the contract; the canvas is just its picture.
    await expect
      .poll(async () => {
        const state = await page.request.get("/api/state").then((r) => r.json());
        return [state.device_sets.length, state.device_sets[0]?.channels?.length ?? 0];
      })
      .toEqual([1, 1]);

    // Pinning moves the live face to the rack and leaves a placeholder behind (CANVAS §5).
    await node("scope")
      .getByRole("button", { name: /pin to the rack/i })
      .click();
    await expect(node("scope").getByText(/pinned to the rack/i)).toBeVisible();

    const rack = page.getByRole("group", { name: "View" }).getByRole("button", { name: "Rack" });
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);

    // The arrangement is server state, not browser state (PLAN §10): a reload restores it.
    await page.reload();
    await expect(node("scope").getByText(/pinned to the rack/i)).toBeVisible();
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);
  });
});
