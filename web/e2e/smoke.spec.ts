// The station, end to end: open the app, bind the virtual radio to the receiver node, add a
// channel, hear the graph become a running station, pin a face and find it on the rack.
//
// This is the one test that exercises the composition — canvas, faces, WebSocket, apply — that
// the unit suite cannot reach (PLAN §14). It asserts behaviour, not markup: what an operator
// would check after each gesture.
import { expect, type Locator, type Page, test } from "@playwright/test";
// The state shape is generated from the server's OpenAPI, like everywhere else (CLAUDE.md #1).
import type { StateSnapshot } from "../src/lib/types";

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

/** The rack as the server has it — the arrangement is server state, not what the DOM happens to
 * be showing mid-gesture. */
async function slots(page: Page): Promise<{ node: string; x: number; w: number }[]> {
  const list = await page.request.get("/api/workspaces").then((r) => r.json());
  const detail = await page.request.get(`/api/workspaces/${list.active}`).then((r) => r.json());
  return detail.snapshot.rack.slots;
}

/** Drag a rack grip by whole cells. The grid is `RACK_COLS` wide, so a cell is the container's
 * width over twelve — the same arithmetic the rack itself does. */
async function dragBy(page: Page, grip: Locator, cells: number): Promise<void> {
  const box = await grip.boundingBox();
  const grid = await page.locator(".grid").first().boundingBox();
  if (box === null || grid === null) {
    throw new Error("a grip to drag and a grid to drag it in");
  }
  const step = (grid.width / 12) * cells;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + step, box.y + box.height / 2, { steps: 8 });
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
        const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
        return [state.device_sets.length, state.device_sets[0]?.channels.length ?? 0];
      })
      .toEqual([1, 1]);

    // Cycling the mode has to move the node and its engine channel together: the node names the
    // type (CANVAS §4), so a patch left naming the old one unbinds the face and the next apply
    // adds a second channel for it.
    const channel = page.locator('.react-flow__node[data-id^="channel:"]');
    await channel.getByText("NFM", { exact: true }).first().click();
    await page.keyboard.press("m");
    await expect
      .poll(async () => {
        const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
        return state.device_sets[0]?.channels.map((c) => c.settings.params.type) ?? [];
      })
      .toEqual(["wfm"]);
    // Still one face, still bound — not the "not created" state a desynced node falls into.
    await expect(channel).toHaveCount(1);
    await expect(channel.getByText(/nothing feeds this channel|not been created/i)).toHaveCount(0);

    // Pinning adds the face to the rack and leaves the canvas node where it was (CANVAS §5).
    for (const id of ["scope", "speaker"]) {
      await node(id)
        .getByRole("button", { name: /pin to the rack/i })
        .click();
      await expect(node(id).getByRole("button", { name: /unpin from the rack/i })).toBeVisible();
    }
    await expect(node("scope").getByText(/pinned to the rack/i)).toHaveCount(0);

    const rack = page.getByRole("group", { name: "View" }).getByRole("button", { name: "Rack" });
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);

    // Dragging the boundary between two faces makes one larger and the other smaller (CANVAS §5).
    // The whole point of the gesture is that it re-balances a full rack without a hole, so both
    // halves are asserted — and against the *stored* rack, since that is what survives a reload.
    const before = await slots(page);
    await dragBy(page, page.locator('[title="Drag the boundary to the right"]').first(), 1);
    await expect
      .poll(async () => (await slots(page)).map((slot) => slot.w))
      .toEqual([(before[0]?.w ?? 0) + 1, (before[1]?.w ?? 0) - 1]);

    // The arrangement is server state, not browser state (PLAN §10): a reload restores it — and
    // the station comes back bound, which is what applying on load buys.
    await page.reload();
    await expect(node("scope").getByRole("button", { name: /unpin from the rack/i })).toBeVisible();
    await expect(node("device").locator('[id^="frequency-dial"]')).toBeVisible();
    await expect(
      page.locator('.react-flow__node[data-id^="channel:"]').getByText(/nothing feeds/i),
    ).toHaveCount(0);
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);
  });
});
