import { expect, type Locator, type Page, test } from "@playwright/test";
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

/** A popup inside a zoomed patch must inherit the same physical scale as its field. */
async function renderedScale(locator: Locator): Promise<number> {
  return locator.evaluate((element) => {
    const height = (element as HTMLElement).offsetHeight;
    return height === 0 ? 1 : element.getBoundingClientRect().height / height;
  });
}

/** Leave an autocomplete field the way a pointer does: press somewhere else on the face. While a
 * suggestion popup is open it hides every element outside itself from the accessibility tree, and
 * a scripted `blur()` reports no element focus moved to — which a combobox reads as the window
 * losing focus, not the field being left — so the popup would stay open over the next control. */
async function leaveField(node: Locator): Promise<void> {
  await node.locator("header").click();
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
    await page.route("**/api/devices", (route) =>
      route.fulfill({
        json: {
          devices: [
            { driver: "virtual", key: "siggen", label: "Signal Generator (virtual)" },
            ...Array.from({ length: 100 }, (_, index) => ({
              driver: "virtual",
              key: `file:/recordings/capture-${index.toString().padStart(3, "0")}`,
              label: `capture-${index.toString().padStart(3, "0")} (recording)`,
            })),
          ],
        },
      }),
    );
    // MapLibre rejects a paint colour it cannot parse by dropping the whole layer with one
    // console error — the map then looks whole minus its targets, which no locator below sees.
    const styleErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error" && message.text().includes("color expected")) {
        styleErrors.push(message.text());
      }
    });
    await page.goto("/");

    const node = (id: string) => page.locator(`.react-flow__node[data-id="${id}"]`);
    const receiver = node("device");
    await expect(receiver).toBeVisible();
    await expect(node("scope")).toBeVisible();
    await expect(node("speaker")).toBeVisible();

    const recordings = receiver.getByRole("button", { name: "Recordings (100)" });
    await expect(receiver.getByRole("button", { name: /capture-099/i })).toHaveCount(0);
    await recordings.click();
    const recordingsDialog = page.getByRole("dialog", { name: "Recordings" });
    await expect(recordingsDialog).toBeVisible();
    await recordingsDialog.getByRole("searchbox", { name: "Search recordings" }).fill("099");
    await expect(recordingsDialog.getByRole("button", { name: /capture-099/i })).toBeVisible();
    await expect(recordingsDialog.getByRole("button", { name: /capture-000/i })).toHaveCount(0);
    await recordingsDialog.getByRole("button", { name: "Close" }).click();
    await expect(recordings).toBeFocused();

    await receiver.getByRole("button", { name: /signal generator/i }).click();
    await expect(receiver.locator('[id^="frequency-dial"]')).toBeVisible();

    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "NFM", exact: true }).click();
    const channel = page.locator('.react-flow__node[data-id^="channel:"]');
    await expect(channel).toBeVisible();

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

    await channel.getByText("NFM", { exact: true }).first().click();
    await page.keyboard.press("m");
    await expect
      .poll(async () => {
        const state: StateSnapshot = await page.request.get("/api/state").then((r) => r.json());
        return state.device_sets[0]?.channels.map((c) => c.settings.params.type) ?? [];
      })
      .toEqual(["wfm"]);
    await expect(channel).toHaveCount(1);
    await expect(channel.getByText(/nothing feeds this channel|not been created/i)).toHaveCount(0);

    await expect(channel.locator('.react-flow__handle[data-handleid="video"]')).toHaveCount(0);
    for (const port of ["iq", "audio"]) {
      expect(await rowOffset(channel, port)).toBeLessThan(1);
    }

    // The threshold is drawn whether or not squelch is on — switching it must not resize the face
    // under the pointer — so "off" is the slider going inert, not the row disappearing.
    const squelch = channel.getByRole("checkbox", { name: /squelch/i });
    const threshold = channel.getByRole("slider", { name: /squelch threshold/i });
    await expect(threshold).toBeDisabled();
    await squelch.click();
    await expect(threshold).toBeEnabled();
    expect(await cursor(squelch)).toBe("pointer");
    expect(await cursor(threshold.locator("xpath=.."))).toBe("grab");
    await channel.getByText("-60 dB", { exact: true }).click();
    await expect(squelch).toBeChecked();
    await squelch.click();
    await expect(threshold).toBeDisabled();

    await expect(node("scope").getByText(/waiting for the first frame/i)).toHaveCount(0);

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
    await expect(rack).toHaveText(/2/);
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);

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

    await scopePlot.getByRole("button", { name: /^traces$/i }).click();
    const tracesDialog = page.getByRole("dialog");
    const peak = tracesDialog.getByRole("button", { name: /^peak$/i });
    await peak.click();
    await expect(peak).toHaveAttribute("aria-pressed", "true");
    await peak.click();
    await expect(peak).toHaveAttribute("aria-pressed", "false");
    await page.keyboard.press("Escape");
    await expect(tracesDialog).toBeHidden();

    // The trigger is labelled with the colormap in force, which on a fresh profile is the default.
    await scopePlot.getByRole("button", { name: /^classic$/i }).click();
    await page.getByRole("button", { name: /^viridis$/i }).click();
    await expect(
      scopePlot.locator('button[aria-haspopup="dialog"]', { hasText: /^viridis$/i }),
    ).toBeVisible();

    expect(await tunedTo()).toBe(tuning);

    const before = await slots(page);
    await dragBy(page, page.locator('[title="Drag the boundary to the right"]').first(), 1);
    await expect
      .poll(async () => (await slots(page)).map((slot) => slot.w))
      .toEqual([(before[0]?.w ?? 0) + 1, (before[1]?.w ?? 0) - 1]);

    await page.reload();
    await expect(node("scope").getByRole("button", { name: /unpin from the rack/i })).toBeVisible();
    await expect(node("device").locator('[id^="frequency-dial"]')).toBeVisible();
    await expect(
      page.locator('.react-flow__node[data-id^="channel:"]').getByText(/nothing feeds/i),
    ).toHaveCount(0);
    await rack.click();
    await expect(page.getByText(/nothing pinned/i)).toHaveCount(0);

    await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Patch" }).click();
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
    expect(styleErrors).toEqual([]);
    const mapId = await map.getAttribute("data-id");
    if (mapId === null) {
      throw new Error("a map node id");
    }
    await expect
      .poll(async () => {
        const list = await page.request.get("/api/workspaces").then((r) => r.json());
        const detail: WorkspaceDetail = await page.request
          .get(`/api/workspaces/${list.active}`)
          .then((r) => r.json());
        return (detail.snapshot.graph.edges ?? []).some((edge) => edge.to.node === mapId);
      })
      .toBe(true);
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

    const map = page.locator('.react-flow__node[data-id^="map:"]', { hasText: "Aircraft" });
    await fitPatch(page);
    const attribution = map.locator(".maplibregl-ctrl-attrib");

    await expect(attribution).toBeVisible();
    await expect(attribution.locator(".maplibregl-ctrl-attrib-inner")).toBeHidden();

    await map.locator("header").click();
    await attribution.locator("summary.maplibregl-ctrl-attrib-button").click();
    await expect(attribution.getByText("Stub basemap credits")).toBeVisible();
  });

  test("configures NMEA GPS and renders a live device fix", async ({ page }) => {
    await page.route("**/api/position/nmea-devices", (route) =>
      route.fulfill({
        json: {
          devices: [
            {
              path: "/dev/cu.usbmodem11401",
              product: "u-blox GNSS receiver",
              manufacturer: "u-blox",
              serial: "GNSS-1",
            },
          ],
        },
      }),
    );
    await page.goto("/");
    await expect(page.locator('.react-flow__node[data-id="device"]')).toBeVisible();

    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "NMEA serial" }).click();
    const nmea = page.locator('.react-flow__node[data-id^="gps:"]', { hasText: "NMEA" });
    await expect(nmea).toBeVisible();

    const device = nmea.getByRole("combobox", { name: "Serial device" });
    await device.fill("");
    await device.click();
    const detectedDevice = page.getByRole("option", { name: /\/dev\/cu\.usbmodem11401/ });
    await expect(detectedDevice).toBeVisible();
    const [fieldScale, popupScale] = await Promise.all([
      renderedScale(device),
      renderedScale(detectedDevice),
    ]);
    expect(Math.abs(fieldScale - popupScale)).toBeLessThan(0.05);
    await detectedDevice.click();
    await expect(device).toHaveValue("/dev/cu.usbmodem11401");
    await device.fill("/dev/ttyACM7");
    await leaveField(nmea);
    const baud = nmea.getByRole("combobox", { name: "Baud" });
    await baud.fill("38400");
    await leaveField(nmea);
    await nmea.getByRole("combobox", { name: "Update rate" }).click();
    await page.getByRole("option", { name: "5 Hz" }).click();
    await device.fill(" ");
    await leaveField(nmea);
    await expect(device).toHaveValue("/dev/ttyACM7");
    await baud.fill("100");
    await leaveField(nmea);
    await expect(baud).toHaveValue("38400");

    await expect
      .poll(async () => {
        const list = await page.request.get("/api/workspaces").then((response) => response.json());
        const detail: WorkspaceDetail = await page.request
          .get(`/api/workspaces/${list.active}`)
          .then((response) => response.json());
        return detail.snapshot.graph.nodes
          .filter((node) => node.kind === "gps")
          .map((node) => JSON.stringify(node.data.source));
      })
      .toContain(
        JSON.stringify({
          type: "nmea",
          device: "/dev/ttyACM7",
          baud: 38_400,
          update_interval_ms: 200,
        }),
      );

    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "GPSD" }).click();
    const gpsd = page.locator('.react-flow__node[data-id^="gps:"]', { hasText: "gpsd" });
    const address = gpsd.getByRole("textbox", { name: "GPSD address" });
    await address.fill("not-an-endpoint");
    await address.blur();
    await expect(address).toHaveValue("127.0.0.1:2947");

    await page.getByRole("button", { name: "+ Node" }).click();
    await page.getByRole("button", { name: "Device GPS" }).click();
    let deviceNode = "";
    await expect
      .poll(async () => {
        const list = await page.request.get("/api/workspaces").then((response) => response.json());
        const detail: WorkspaceDetail = await page.request
          .get(`/api/workspaces/${list.active}`)
          .then((response) => response.json());
        deviceNode =
          detail.snapshot.graph.nodes.find(
            (node) => node.kind === "gps" && node.data.source?.type === "device",
          )?.id ?? "";
        return deviceNode;
      })
      .not.toBe("");
    await page.evaluate(async (node) => {
      await new Promise<void>((resolve, reject) => {
        const socket = new WebSocket(`ws://${window.location.host}/api/ws`);
        let published = false;
        let finished = false;
        const publishFix = (): void => {
          if (finished || socket.readyState !== WebSocket.OPEN) {
            return;
          }
          published = true;
          socket.send(
            JSON.stringify({
              type: "PublishPosition",
              data: {
                node,
                fix: {
                  latitude: 52.52,
                  longitude: 13.405,
                  accuracy_m: 4,
                  time: "2026-08-14T12:00:00Z",
                },
              },
            }),
          );
        };
        const timeout = window.setTimeout(() => {
          socket.close();
          reject(new Error("position subscription was not ready"));
        }, 10_000);
        socket.onerror = () => {
          window.clearTimeout(timeout);
          reject(new Error("position test socket failed"));
        };
        socket.onmessage = (message) => {
          const event = JSON.parse(String(message.data));
          if (event.type === "Hello" && !published) {
            publishFix();
            return;
          }
          if (event.type === "Error" && published) {
            published = false;
            window.setTimeout(publishFix, 75);
            return;
          }
          if (
            event.type === "PositionChanged" &&
            event.data.node === node &&
            event.data.fix?.latitude === 52.52
          ) {
            finished = true;
            window.clearTimeout(timeout);
            socket.close();
            resolve();
          }
        };
      });
    }, deviceNode);
    const deviceGps = page.locator(`.react-flow__node[data-id="${deviceNode}"]`);
    await expect(deviceGps.getByText("52.520000, 13.405000")).toBeVisible();
    await expect(deviceGps.getByText("JO62qm")).toBeVisible();
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

    await ruler.click();
    await expect(ruler).toBeChecked();
  });

  test("runs a tool beside the receiver without touching the patch", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('.react-flow__node[data-id="device"]')).toBeVisible();

    await page.getByRole("button", { name: "Tools", exact: true }).click();
    const tools = page.getByRole("dialog", { name: "Antenna calculator" });
    await expect(tools).toBeVisible();

    const frequency = tools.getByRole("textbox", { name: "Frequency in MHz" });
    await frequency.click();
    await frequency.press("ControlOrMeta+a");
    await frequency.pressSequentially("14.2");
    await frequency.press("Tab");
    await expect(frequency).toHaveValue("14.2");
    // A half-wave dipole at 14.2 MHz, end-effect factor 0.95: the whole path — panel to server
    // to table — in one number.
    await expect(tools.getByRole("row", { name: /tip-to-tip span/i })).toContainText(/10\.0\d\d m/);
    // The same answer as a drawing: a flat dipole is drawn face on.
    await expect(tools.getByRole("img", { name: /dipole.*front view/i })).toBeVisible();

    await tools.getByRole("combobox", { name: "Antenna design" }).click();
    await page.getByRole("option", { name: "Yagi" }).click();
    const directors = tools.getByRole("textbox", { name: "Director count" });
    await directors.click();
    await directors.press("ControlOrMeta+a");
    await directors.pressSequentially("3");
    // Enter commits without leaving the field: the panel answers while the operator is still in it.
    await directors.press("Enter");
    await expect(tools.getByRole("row", { name: /director 3/i })).toBeVisible();
    // A boom is only visible from above, and every element on it gets drawn.
    const drawing = tools.getByRole("img", { name: /yagi.*top view/i });
    await expect(drawing).toBeVisible();
    await expect(drawing.locator("title", { hasText: /^Director 3 —/ })).toHaveCount(1);

    await tools.getByRole("group", { name: "Drawing view" }).getByText("3D").click();
    await expect(tools.getByRole("img", { name: /yagi.*angle/i })).toBeVisible();
    await expect(tools.getByRole("button", { name: "Reset angle" })).toBeVisible();

    await tools.getByRole("group", { name: "Length units" }).getByText("ft").click();
    await expect(tools.getByRole("row", { name: /^Reflector\b/ })).toContainText(/ft/);

    await tools.getByRole("button", { name: "Close" }).click();
    await expect(tools).toHaveCount(0);
    // The patch is exactly where it was: a tool is not part of the signal path.
    await expect(page.locator('.react-flow__node[data-id="device"]')).toBeVisible();
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
    await expect(page.locator('header img[src="/icon.svg"]')).toBeVisible();
  });
});
