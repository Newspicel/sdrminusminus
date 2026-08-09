import { describe, expect, it } from "vitest";
import type { LayoutNode, PanelKind, TabSpec } from "../lib/types";
import {
  type DockSize,
  fromSerializedDockview,
  normalize,
  sameTab,
  tabPanels,
  toSerializedDockview,
} from "./dockLayout";

const SIZE: DockSize = { width: 1600, height: 900 };

function group(...kinds: PanelKind[]): LayoutNode {
  return {
    node: "group",
    data: { panels: kinds.map((kind) => ({ id: `panel:${kind}`, kind })) },
  };
}

function tab(layout: LayoutNode, floating: TabSpec["floating"] = []): TabSpec {
  return { id: "station", name: "Station", layout, floating };
}

/** The layout `WorkspaceSnapshot::station_default()` seeds, written by hand so a change on the
 * Rust side that this mapper cannot represent shows up as a failure here. */
const STATION: LayoutNode = {
  node: "split",
  data: {
    direction: "column",
    children: [
      { weight_permille: 600, node: group("spectrum") },
      {
        weight_permille: 400,
        node: {
          node: "split",
          data: {
            direction: "row",
            children: [
              { weight_permille: 600, node: group("channels", "scanner") },
              {
                weight_permille: 400,
                node: group("presets", "bookmarks", "templates", "recordings"),
              },
            ],
          },
        },
      },
    ],
  },
};

describe("dock layout mapping", () => {
  it("round-trips the default station layout unchanged", () => {
    const start = tab(STATION);
    const back = fromSerializedDockview(toSerializedDockview(start, SIZE), start);
    expect(back).toEqual(start);
    expect(sameTab(back, start)).toBe(true);
  });

  /// A second pass must be a fixed point, or every load→save cycle would rewrite the layout and
  /// fan a state change to every client for nothing.
  it("is idempotent across repeated round trips", () => {
    const start = tab(STATION);
    const once = fromSerializedDockview(toSerializedDockview(start, SIZE), start);
    const twice = fromSerializedDockview(toSerializedDockview(once, SIZE), once);
    expect(twice).toEqual(once);
  });

  it("keeps the active panel of a group and a renamed title", () => {
    const start = tab({
      node: "group",
      data: {
        panels: [
          { id: "panel:channels", kind: "channels" },
          { id: "panel:scanner", kind: "scanner", title: "Sweep" },
        ],
        active: "panel:scanner",
      },
    });
    const back = fromSerializedDockview(toSerializedDockview(start, SIZE), start);
    expect(back).toEqual(start);
  });

  /// dockview writes pixels; the model stores permille. A layout dragged at one width must open
  /// at another width in the same proportions.
  it("reads pixel sizes back as permille shares", () => {
    const start = tab(STATION);
    const serialized = toSerializedDockview(start, SIZE);
    const root = serialized.grid.root;
    expect(root.type).toBe("branch");
    // Simulate the dock having laid the split out at real pixels: 300 / 600 of the height.
    const children = root.data as { size?: number }[];
    expect(children).toHaveLength(2);
    if (children[0] && children[1]) {
      children[0].size = 300;
      children[1].size = 600;
    }
    const back = fromSerializedDockview(serialized, start);
    expect(back.layout.node).toBe("split");
    if (back.layout.node === "split") {
      expect(back.layout.data.children.map((c) => c.weight_permille)).toEqual([333, 667]);
    }
  });

  it("maps floating groups through viewport fractions, including corner anchors", () => {
    const start = tab(group("spectrum"), [
      {
        group: { panels: [{ id: "panel:map", kind: "map" }] },
        x_frac: 0.25,
        y_frac: 0.5,
        w_frac: 0.5,
        h_frac: 0.25,
      },
    ]);
    const serialized = toSerializedDockview(start, SIZE);
    expect(serialized.floatingGroups?.[0]?.position).toEqual({
      left: 400,
      top: 450,
      width: 800,
      height: 225,
    });
    expect(fromSerializedDockview(serialized, start)).toEqual(start);

    // dockview anchors a floating group to whichever corner it was dragged nearest; the mapper
    // must resolve every form to the same top-left fraction.
    const anchored = toSerializedDockview(start, SIZE);
    const floating = anchored.floatingGroups?.[0];
    if (floating) {
      floating.position = { right: 400, bottom: 225, width: 800, height: 225 };
    }
    const back = fromSerializedDockview(anchored, start);
    // Same rectangle, expressed from the far corners: left 1600−400−800, top 900−225−225.
    expect(back.floating?.[0]?.x_frac).toBeCloseTo(0.25, 5);
    expect(back.floating?.[0]?.y_frac).toBeCloseTo(0.5, 5);
  });

  /// dockview's grid alternates axis at every depth, so a row inside a row cannot exist: it is
  /// flattened on the way in, and the child's shares are folded into the parent's.
  it("collapses same-direction nesting and folds the weights", () => {
    const nested: LayoutNode = {
      node: "split",
      data: {
        direction: "row",
        children: [
          { weight_permille: 500, node: group("spectrum") },
          {
            weight_permille: 500,
            node: {
              node: "split",
              data: {
                direction: "row",
                children: [
                  { weight_permille: 800, node: group("map") },
                  { weight_permille: 200, node: group("channels") },
                ],
              },
            },
          },
        ],
      },
    };
    const flat = normalize(nested);
    expect(flat.node).toBe("split");
    if (flat.node === "split") {
      expect(flat.data.children.map((c) => c.weight_permille)).toEqual([500, 400, 100]);
      expect(flat.data.children).toHaveLength(3);
    }
  });

  it("drops empty groups and degenerate splits", () => {
    const degenerate: LayoutNode = {
      node: "split",
      data: {
        direction: "row",
        children: [
          { weight_permille: 500, node: { node: "group", data: { panels: [] } } },
          { weight_permille: 500, node: group("map") },
        ],
      },
    };
    expect(normalize(degenerate)).toEqual(group("map"));
  });

  /// A component name that is not a panel kind (an older build's panel, a hand-edited row)
  /// cannot be rendered, so it is dropped rather than written back as an unrenderable panel.
  it("ignores panels whose component is not a known kind", () => {
    const start = tab(group("spectrum"));
    const serialized = toSerializedDockview(start, SIZE);
    serialized.panels["panel:ghost"] = { id: "panel:ghost", contentComponent: "ghost" };
    const [wrapped] = serialized.grid.root.data as { data: { views: string[] } }[];
    if (wrapped) {
      wrapped.data.views = [...wrapped.data.views, "panel:ghost"];
    }
    const back = fromSerializedDockview(serialized, start);
    expect(tabPanels(back).map((p) => p.id)).toEqual(["panel:spectrum"]);
  });

  /// dockview refuses a serialized layout whose root is a leaf, which is what a one-group tab
  /// (and every tab in narrow mode) compiles to.
  it("wraps a single group so the dock accepts the root, and unwraps it again", () => {
    const start = tab(group("spectrum", "channels"));
    const serialized = toSerializedDockview(start, SIZE);
    expect(serialized.grid.root.type).toBe("branch");
    expect(fromSerializedDockview(serialized, start)).toEqual(start);
  });

  it("survives an empty dock without inventing a layout", () => {
    const start = tab(group("spectrum"));
    const back = fromSerializedDockview({ grid: undefined, panels: {} } as never, start);
    expect(back.layout).toEqual({ node: "group", data: { panels: [] } });
    expect(back.id).toBe("station");
  });
});
