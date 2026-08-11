// The workspace, end to end: open the app, bind the virtual radio to the receiver node, add a
// channel, hear the graph become a running workspace, pin a face and find it on the rack.
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

/** Bring every node into view. New nodes drop to the right of everything already drawn, which
 * after a few adds is outside the framed viewport — and a wire cannot be dragged to a handle
 * the pointer cannot reach. */
async function fitPatch(page: Page): Promise<void> {
  const pane = page.locator(".react-flow__pane");
  const box = await pane.boundingBox();
  if (box === null) {
    throw new Error("a pane to right-click");
  }
  await page.mouse.click(box.x + 40, box.y + box.height - 40, { button: "right" });
  await page
    .getByRole("menu")
    .getByRole("button", { name: /fit the patch/i })
    .click();
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

test.describe("the workspace", () => {
  test("binds a radio, adds a channel and pins a face", async ({ page }) => {
    // The tile CDN is cut off, not awaited: CI must not lean on a third party, and the offline
    // fallback the map leg below lands in is itself behaviour the map owes a field workspace.
    await page.route("https://tiles.openfreemap.org/**", (route) => route.abort());
    // MapLibre rejects a paint colour it cannot parse by dropping the whole layer with one
    // console error — the map then looks whole minus its targets, which no locator below sees.
    const styleErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error" && message.text().includes("color expected")) {
        styleErrors.push(message.text());
      }
    });
    await page.goto("/");

    // The default workspace: a receiver node with nothing in it, a scope and a speaker. Nodes are
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

    // The scope is running before the view switch below, which is what gives that switch
    // something to preserve.
    await expect(node("scope").getByText(/waiting for the first frame/i)).toHaveCount(0);

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

    // A view switch remounts every face, and a plot's history is its own (gl/waterfall.ts), so the
    // scope used to come back empty. It opens on the lane's kept rows and its last readout instead.
    // Read once rather than polled: waiting for the readout would be waiting for the very frame
    // that used to hide the gap.
    expect(await page.getByText(/\d\.\d{4} MHz/).count()).toBeGreaterThan(0);

    // Dragging the boundary between two faces makes one larger and the other smaller (CANVAS §5).
    // The whole point of the gesture is that it re-balances a full rack without a hole, so both
    // halves are asserted — and against the *stored* rack, since that is what survives a reload.
    const before = await slots(page);
    await dragBy(page, page.locator('[title="Drag the boundary to the right"]').first(), 1);
    await expect
      .poll(async () => (await slots(page)).map((slot) => slot.w))
      .toEqual([(before[0]?.w ?? 0) + 1, (before[1]?.w ?? 0) - 1]);

    // The arrangement is server state, not browser state (PLAN §10): a reload restores it — and
    // the workspace comes back bound, which is what applying on load buys.
    await page.reload();
    await expect(node("scope").getByRole("button", { name: /unpin from the rack/i })).toBeVisible();
    await expect(node("device").locator('[id^="frequency-dial"]')).toBeVisible();
    await expect(
      page.locator('.react-flow__node[data-id^="channel:"]').getByText(/nothing feeds/i),
    ).toHaveCount(0);
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);

    // The map leg: what a map node plots is decided by its wires (CANVAS §1), so it needs a
    // position-reporting decoder wired in — ADS-B, which decodes only at exactly 2 Msps
    // (PROGRESS.md), a rate the virtual radio offers.
    await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Patch" }).click();
    // A face is a picture until it is clicked (NodeShell): the first click selects the node and
    // only then does the face answer its own pointer, so the select needs the header click first.
    await node("device").locator("header").click();
    await node("device").getByRole("combobox", { name: "Sample rate" }).click();
    await page.getByRole("option", { name: "2.000 MS/s" }).click();

    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "ADS-B (1090ES)" }).click();
    const adsb = page.locator('.react-flow__node[data-id^="channel:"]', { hasText: "ADS-B" });
    await fitPatch(page);
    await dragWire(
      page,
      receiver.locator('.react-flow__handle[data-handleid="iq"]'),
      adsb.locator('.react-flow__handle[data-handleid="iq"]'),
    );

    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "Map", exact: true }).click();
    const map = page.locator('.react-flow__node[data-id^="map:"]');
    await fitPatch(page);
    await dragWire(
      page,
      adsb.locator('.react-flow__handle[data-handleid="events"]'),
      map.locator('.react-flow__handle[data-handleid="events"]'),
    );

    // The face proves the composition: the wire chose the aircraft layer (the legend), the
    // aborted CDN landed the offline fallback, and the canvas has real height — the break this
    // guards collapsed the container to zero and left MapLibre painting into a box nobody saw.
    await expect(map.getByText("Aircraft")).toBeVisible();
    await expect(map.getByText(/basemap unavailable/i)).toBeVisible();
    await expect
      .poll(async () => (await map.locator(".maplibregl-canvas").boundingBox())?.height ?? 0)
      .toBeGreaterThan(0);
    // Last: by the time the canvas above measured, `style.load` had installed the target
    // layers, which is where a rejected paint colour would have said so.
    expect(styleErrors).toEqual([]);
  });
});
