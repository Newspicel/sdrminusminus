// The workspace, end to end: open the app, bind the virtual radio to the receiver node, add a
// channel, hear the graph become a running workspace, pin a face and find it on the rack.
//
// This is the one suite that exercises the composition — canvas, faces, WebSocket, apply — that
// the unit suite cannot reach (PLAN §14). It asserts behaviour, not markup: what an operator
// would check after each gesture. The flow above runs first and the legs below build on the
// workspace it leaves, which the single worker and the throwaway database below make sound.
import { expect, type Locator, type Page, test } from "@playwright/test";
// The state shape is generated from the server's OpenAPI, like everywhere else (CLAUDE.md #1).
import type { StateSnapshot, WorkspaceDetail } from "../src/lib/types";

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

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

/** How far a port's marker sits from the middle of its own label, in pixels. Both are placed from
 * the same `top`, so anything above a rounding error is a marker that has moved under its paint. */
async function rowOffset(node: Locator, port: string): Promise<number> {
  const handle = `.react-flow__handle[data-handleid="${port}"]`;
  const marker = await node.locator(handle).boundingBox();
  // The label is the handle's own sibling — the face's body has text of its own, and some of it
  // is the same word.
  const label = await node.locator(`${handle} + span`).boundingBox();
  if (marker === null || label === null) {
    throw new Error(`a ${port} port with a label`);
  }
  return Math.abs(marker.y + marker.height / 2 - (label.y + label.height / 2));
}

/** The cursor a control actually paints. Computed, not the class list: the rule that gives a
 * headless primitive its pointer lives in the stylesheet, and a node's own `cursor` inherits
 * down over anything that fails to state one. */
async function cursor(locator: Locator): Promise<string> {
  return locator.evaluate((element) => getComputedStyle(element).cursor);
}

/** One face in the rack. The rack has no wires and no pane, so its faces are addressed by the
 * node they render rather than through React Flow. */
function rackNode(page: Page, id: string): Locator {
  return page.locator(`.grid > [data-id="${id}"]`);
}

/** The rack as the server has it — the arrangement is server state, not what the DOM happens to
 * be showing mid-gesture. */
async function slots(page: Page): Promise<{ node: string; x: number; w: number }[]> {
  const list = await page.request.get("/api/workspaces").then((r) => r.json());
  const detail = await page.request.get(`/api/workspaces/${list.active}`).then((r) => r.json());
  return detail.snapshot.rack.slots;
}

/** Bring every node into view before wiring faces from opposite sides of a large patch. */
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
  // These legs deliberately share the throwaway server state built by the first one. If that
  // setup fails, later assertions describe consequences rather than independent failures.
  test.describe.configure({ mode: "serial" });

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
    const channel = page.locator('.react-flow__node[data-id^="channel:"]');
    await expect(channel).toBeVisible();

    // A palette add belongs to the camera the operator is looking through, not to the graph's
    // rightmost coordinate. Its rendered face is wholly reachable and, while this starter patch
    // has room, does not cover any face already there.
    const canvasBounds = await page.locator(".react-flow").boundingBox();
    const channelBounds = await channel.boundingBox();
    if (canvasBounds === null || channelBounds === null) {
      throw new Error("a visible canvas and newly added channel");
    }
    expect(channelBounds.x).toBeGreaterThanOrEqual(canvasBounds.x);
    expect(channelBounds.y).toBeGreaterThanOrEqual(canvasBounds.y);
    expect(channelBounds.x + channelBounds.width).toBeLessThanOrEqual(
      canvasBounds.x + canvasBounds.width,
    );
    expect(channelBounds.y + channelBounds.height).toBeLessThanOrEqual(
      canvasBounds.y + canvasBounds.height,
    );
    for (const id of ["device", "scope", "speaker"]) {
      const existing = await node(id).boundingBox();
      if (existing === null) {
        throw new Error(`a visible ${id} node`);
      }
      const overlapWidth =
        Math.min(channelBounds.x + channelBounds.width, existing.x + existing.width) -
        Math.max(channelBounds.x, existing.x);
      const overlapHeight =
        Math.min(channelBounds.y + channelBounds.height, existing.y + existing.height) -
        Math.max(channelBounds.y, existing.y);
      expect(overlapWidth > 0 && overlapHeight > 0).toBe(false);
    }

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

    // A conditional port belongs to the channel types that have it and to no other: WFM scans out
    // no picture, so there is nothing on this face to wire a screen to.
    await expect(channel.locator('.react-flow__handle[data-handleid="video"]')).toHaveCount(0);
    // Each marker sits on its own row, which is the only thing pairing it with its label — a
    // marker drawn through a transform of its own drifts off the line it names.
    for (const port of ["iq", "audio"]) {
      expect(await rowOffset(channel, port)).toBeLessThan(1);
    }

    // The squelch row: the box and its word are the label, the threshold beside them is not. A
    // row that labelled the whole line forwarded a click on the readout to the box and turned
    // the gate off. The cursors are the other half of the same claim — what acts says so, and
    // says which way it acts (DESIGN.md §4, `index.css`).
    const squelch = channel.getByRole("checkbox", { name: /squelch/i });
    await squelch.click();
    const threshold = channel.getByRole("slider", { name: /squelch threshold/i });
    await expect(threshold).toBeAttached();
    expect(await cursor(squelch)).toBe("pointer");
    expect(await cursor(threshold.locator("xpath=.."))).toBe("grab");
    await channel.getByText("-60", { exact: true }).click();
    await expect(squelch).toBeChecked();
    // Left as the leg found it: the legs below run on this workspace.
    await squelch.click();
    await expect(threshold).toHaveCount(0);

    // The scope is running before the view switch below, which is what gives that switch
    // something to preserve.
    await expect(node("scope").getByText(/waiting for the first frame/i)).toHaveCount(0);

    // Hold the one-pin refetch after the server has answered it. The second optimistic pin lands
    // while that stale response is in flight, deterministically reproducing the ordering that used
    // to erase the second edit before its queued write began.
    const staleGet = deferred();
    const releaseStaleGet = deferred();
    const staleGetFulfilled = deferred();
    let heldStaleGet = false;
    await page.route(/\/api\/workspaces\/\d+$/, async (route) => {
      const request = route.request();
      if (heldStaleGet || request.method() !== "GET") {
        await route.continue();
        return;
      }
      const response = await route.fetch();
      const detail = (await response.json()) as WorkspaceDetail;
      const rackNodes = detail.snapshot.rack?.slots?.map((slot) => slot.node) ?? [];
      if (rackNodes.length !== 1 || rackNodes[0] !== "scope") {
        await route.fulfill({ response });
        return;
      }
      heldStaleGet = true;
      staleGet.resolve();
      await releaseStaleGet.promise;
      await route.fulfill({ response });
      staleGetFulfilled.resolve();
    });

    // Pinning adds the face to the rack and leaves the canvas node where it was (CANVAS §5).
    await node("scope")
      .getByRole("button", { name: /pin to the rack/i })
      .click();
    await expect(node("scope").getByRole("button", { name: /unpin from the rack/i })).toBeVisible();
    await staleGet.promise;
    await node("speaker")
      .getByRole("button", { name: /pin to the rack/i })
      .click();
    await expect(
      node("speaker").getByRole("button", { name: /unpin from the rack/i }),
    ).toBeVisible();
    releaseStaleGet.resolve();
    await staleGetFulfilled.promise;
    await expect
      .poll(async () => (await slots(page)).map((slot) => slot.node))
      .toEqual(["scope", "speaker"]);
    await expect(node("scope").getByText(/pinned to the rack/i)).toHaveCount(0);

    const rack = page.getByRole("group", { name: "View" }).getByRole("button", { name: "Rack" });
    // What is on the rack is answerable from the patch view: the button counts it, so pinning
    // has an outcome you can see without switching to look for it.
    await expect(rack).toHaveText(/2/);
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);

    // A view switch remounts every face, and a plot's history is its own (gl/waterfall.ts), so the
    // scope used to come back empty. It opens on the lane's kept rows and its last readout
    // instead. Read once rather than polled: the readout is seeded during the rack's first render
    // (ScopeFace), so polling would wait for the very frame that used to hide the gap.
    expect(
      await rackNode(page, "scope")
        .getByText(/\d\.\d{4} MHz/)
        .count(),
    ).toBeGreaterThan(0);
    await expect(rackNode(page, "scope").getByText(/waiting for the first frame/i)).toHaveCount(0);

    // The plot's own toolbar, in the rack because a face there is always the active one
    // (NodeShell) — so the plot's gestures are armed and this is where they used to swallow it.
    // The plot captures the pointer on `pointerdown` to pan and tune, and a capture on the
    // ancestor retargets the release: the button never saw a click, and the tune-on-click ran
    // instead.
    const scopePlot = rackNode(page, "scope");
    // Where the radio sits: the shared centre and any per-stream override, because `tuneDelta`
    // writes one or the other depending on what the radio scopes per stream.
    const tunedTo = async (): Promise<string> => {
      const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
      const settings = state.device_sets[0]?.settings;
      return JSON.stringify([settings?.center_hz ?? null, settings?.streams ?? []]);
    };
    const tuning = await tunedTo();

    const maxHold = scopePlot.getByRole("button", { name: /max hold/i });
    await maxHold.click();
    await expect(maxHold).toHaveAttribute("aria-pressed", "true");
    await maxHold.click();
    await expect(maxHold).toHaveAttribute("aria-pressed", "false");

    // The trigger is labelled with the colormap in force, so picking one shows in its name.
    await scopePlot.getByRole("button", { name: /^magma$/i }).click();
    await page.getByRole("button", { name: /^viridis$/i }).click();
    await expect(scopePlot.getByRole("button", { name: /^viridis$/i })).toBeVisible();

    // The point of all four clicks: none of them was a click on the *plot*. A radio that moved
    // here is the actual complaint — the buttons were operating the scope.
    expect(await tunedTo()).toBe(tuning);

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

  test("opens the map's basemap credits collapsed", async ({ page }) => {
    // A basemap with credits, which the offline fallback has none of — MapLibre only expands the
    // attribution once a *used* source hands it a line to show, so a source without a layer
    // referencing it would leave the control empty and this test asserting nothing. The tiles
    // themselves stay off the wire; the credit line is the whole subject here.
    await page.route("https://tiles.openfreemap.org/styles/liberty", (route) =>
      route.fulfill({
        json: {
          version: 8,
          sources: {
            basemap: {
              type: "raster",
              tiles: ["https://tiles.openfreemap.org/tiles/{z}/{x}/{y}.png"],
              tileSize: 256,
              attribution: "Stub basemap credits",
            },
          },
          layers: [{ id: "basemap", type: "raster", source: "basemap" }],
        },
      }),
    );
    await page.route("https://tiles.openfreemap.org/tiles/**", (route) => route.abort());
    await page.goto("/");
    await expect(page.locator('.react-flow__node[data-id="device"]')).toBeVisible();

    // The arrangement is server state and the run shares one database, so the map the flow above
    // wired is still on the canvas — and a map is a placeholder until a decoder's events reach it
    // (CANVAS §1), which is what the aircraft legend names.
    const map = page.locator('.react-flow__node[data-id^="map:"]', { hasText: "Aircraft" });
    await fitPatch(page);
    const attribution = map.locator(".maplibregl-ctrl-attrib");

    // Waiting on the control to *appear* is waiting on the credits: MapLibre hides an attribution
    // with nothing in it, and the moment it fills is the moment the old default expanded, since
    // every attribution change re-runs the compact check.
    await expect(attribution).toBeVisible();
    await expect(attribution.locator(".maplibregl-ctrl-attrib-inner")).toBeHidden();

    // Collapsed is a default, not a verdict — the ⓘ still owes the operator the credits. The face
    // only answers a pointer once its node is selected (NodeShell), hence the header click first.
    await map.locator("header").click();
    await attribution.locator("summary.maplibregl-ctrl-attrib-button").click();
    await expect(attribution.getByText("Stub basemap credits")).toBeVisible();
  });

  test("keeps the band plan in the workspace, not in the browser", async ({ page }) => {
    // The region and the ruler moved out of `localStorage` and into the snapshot, so that two
    // operators on one server stop drawing two different rulers over one signal. What proves it
    // is the stored workspace, not the checkbox: the setting has to survive the round trip the
    // canvas's own writes go through.
    await page.goto("/");
    await page
      .getByRole("button", { name: /workspace/i })
      .first()
      .click();
    const ruler = page.getByRole("checkbox", { name: /draw the ruler/i });
    await expect(ruler).toBeChecked();
    await ruler.click();

    await expect
      .poll(async () => {
        const list = await page.request.get("/api/workspaces").then((r) => r.json());
        const detail = await page.request
          .get(`/api/workspaces/${list.active}`)
          .then((r) => r.json());
        return detail.snapshot.settings?.band_ruler;
      })
      .toBe(false);

    // And back, so the leg leaves the workspace as it found it.
    await ruler.click();
    await expect(ruler).toBeChecked();
  });

  test("serves the mark to the tab and the top bar", async ({ page }) => {
    // Both files are rendered from assets/icon.svg by `cargo xtask icons` and reach the binary
    // through `web/dist`, so a missing one is a build that shipped without them. It cannot show
    // up as a 404 either: unknown paths fall back to index.html (server/assets.rs), so the tab
    // would quietly get HTML where it asked for an image. Assert the content type, not the code.
    for (const [path, type] of [
      ["/icon.svg", "image/svg+xml"],
      ["/favicon.ico", "image/"],
    ] as const) {
      const response = await page.request.get(path);
      expect(response.status(), path).toBe(200);
      expect(response.headers()["content-type"], path).toContain(type);
    }

    await page.goto("/");
    // Decorative beside the wordmark, so it has no accessible name to find it by.
    await expect(page.locator('header img[src="/icon.svg"]')).toBeVisible();
  });
});
