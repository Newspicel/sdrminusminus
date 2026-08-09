// Translation between the server's layout tree (`wire::workspace`, PLAN §10) and dockview's
// own serialization. Pure functions, no React, no dockview instance — so the round trip is
// testable, which is the only way to know a drag will not quietly delete a panel.
//
// Two shape differences carry the whole file:
//   1. dockview's grid alternates axis at every depth (a row's child split is always a column),
//      while our tree names a direction per split. Compiling collapses same-direction nesting,
//      which makes alternation an invariant rather than a hope.
//   2. dockview stores pixels; we store permille of the parent. Sizes are fed in as permille and
//      re-normalized on the way back, so a layout laid out at 1600px and reloaded at 1200px is
//      the same layout.

import type { GroupviewPanelState, SerializedDockview, SerializedGridObject } from "dockview-react";
import { Orientation } from "dockview-react";
import type {
  FloatingGroup,
  LayoutNode,
  PanelGroup,
  PanelKind,
  PanelSpec,
  TabSpec,
} from "../lib/types";
import { PANEL_KINDS, panelTitle } from "./panels";

const PERMILLE = 1000;

/** dockview's serialized *group* (a tab-stack: its panel ids and which one is on top). The type
 * is not re-exported from the package root, so it is derived from the shape that is — writing it
 * out by hand would be a second copy of a library type. */
type GroupPanelViewState = NonNullable<
  NonNullable<SerializedDockview["floatingGroups"]>[number]["data"]
>;

export interface DockSize {
  width: number;
  height: number;
}

/** Compile one tab into a layout dockview can restore with `api.fromJSON`. `size` is the dock's
 * current pixel size — the grid is re-laid out to it anyway, but floating groups are positioned
 * in real pixels and would otherwise land off-screen. */
export function toSerializedDockview(tab: TabSpec, size: DockSize): SerializedDockview {
  const root = normalize(tab.layout);
  const panels: Record<string, GroupviewPanelState> = {};
  const groups = new Counter();
  const orientation = rootOrientation(root);
  const written = writeNode(root, panels, groups);
  const serialized: SerializedDockview = {
    grid: {
      // dockview refuses a layout whose root is a leaf ("root must be of type branch"), so a
      // single-group tab — every tab on a phone, and any tab the user closed down to one panel —
      // is wrapped. Reading back collapses the one-child branch again.
      root:
        written.type === "branch" ? written : { type: "branch", data: [written], size: PERMILLE },
      width: Math.max(1, Math.round(size.width)),
      height: Math.max(1, Math.round(size.height)),
      orientation,
    },
    panels,
  };
  const floating = tab.floating ?? [];
  if (floating.length > 0) {
    serialized.floatingGroups = floating.map((group) => ({
      data: writeGroup(group.group, panels, groups),
      position: {
        left: Math.round(group.x_frac * size.width),
        top: Math.round(group.y_frac * size.height),
        width: Math.max(1, Math.round(group.w_frac * size.width)),
        height: Math.max(1, Math.round(group.h_frac * size.height)),
      },
    }));
  }
  return serialized;
}

/** Map dockview's state back onto the tab it came from, keeping the tab's identity (a dock knows
 * nothing about tabs). */
export function fromSerializedDockview(serialized: SerializedDockview, tab: TabSpec): TabSpec {
  const panels = serialized.panels ?? {};
  const size = {
    width: Math.max(1, serialized.grid?.width ?? 1),
    height: Math.max(1, serialized.grid?.height ?? 1),
  };
  const orientation = serialized.grid?.orientation ?? Orientation.HORIZONTAL;
  const layout = serialized.grid?.root
    ? readNode(serialized.grid.root, orientation, panels)
    : emptyGroup();
  const floating: FloatingGroup[] = [];
  for (const group of serialized.floatingGroups ?? []) {
    // Only the single-group form is mapped: a floating *window* hosting its own nested grid is
    // a dockview feature the wire model does not describe, and inventing a placement for it
    // would lose panels. Documented gap (PROGRESS M6) rather than a silent drop of one panel.
    if (!group.data) {
      continue;
    }
    const box = group.position;
    const width = box.width;
    const height = box.height;
    const left = "left" in box ? box.left : size.width - box.right - width;
    const top = "top" in box ? box.top : size.height - box.bottom - height;
    floating.push({
      group: readGroup(group.data, panels),
      x_frac: clampFraction(left / size.width),
      y_frac: clampFraction(top / size.height),
      w_frac: Math.max(0.02, Math.min(1, width / size.width)),
      h_frac: Math.max(0.02, Math.min(1, height / size.height)),
    });
  }
  return { id: tab.id, name: tab.name, layout, floating };
}

/** Structural equality of two tabs. The save path compares before writing: dockview re-emits its
 * layout for changes that do not survive the mapping (a one-pixel sash nudge quantizes to the
 * same permille), and each write would otherwise fan a `StateChanged` to every client. */
export function sameTab(a: TabSpec, b: TabSpec): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Panel ids in a tab, docked and floating — the shell uses it to decide which kinds an "add
 * panel" menu may still offer. */
export function tabPanels(tab: TabSpec): PanelSpec[] {
  const out: PanelSpec[] = [];
  walk(tab.layout, out);
  for (const group of tab.floating ?? []) {
    out.push(...group.group.panels);
  }
  return out;
}

function walk(node: LayoutNode, out: PanelSpec[]): void {
  if (node.node === "split") {
    for (const child of node.data.children) {
      walk(child.node, out);
    }
  } else {
    out.push(...node.data.panels);
  }
}

/** Collapse what dockview cannot represent: same-direction nesting, empty groups, and splits
 * left with fewer than two children. Applied on the way in *and* after reading back, so both
 * directions produce the same canonical tree. */
export function normalize(node: LayoutNode): LayoutNode {
  if (node.node === "group") {
    return node;
  }
  const children: { weight_permille: number; node: LayoutNode }[] = [];
  for (const child of node.data.children) {
    const inner = normalize(child.node);
    const weight = Math.max(1, child.weight_permille);
    if (inner.node === "split" && inner.data.direction === node.data.direction) {
      // The child's weights are shares of the child, which is itself a share of this split.
      const total = sum(inner.data.children.map((c) => Math.max(1, c.weight_permille)));
      for (const grandchild of inner.data.children) {
        children.push({
          weight_permille: Math.max(
            1,
            Math.round((Math.max(1, grandchild.weight_permille) * weight) / total),
          ),
          node: grandchild.node,
        });
      }
      continue;
    }
    if (inner.node === "group" && inner.data.panels.length === 0) {
      continue;
    }
    children.push({ weight_permille: weight, node: inner });
  }
  if (children.length === 0) {
    return emptyGroup();
  }
  if (children.length === 1) {
    return children[0]?.node ?? emptyGroup();
  }
  return {
    node: "split",
    data: { direction: node.data.direction, children: rescale(children) },
  };
}

/** Weights summing to exactly 1000, with the rounding remainder on the last child — so a
 * load→save cycle is a fixed point instead of drifting a permille per pass. */
function rescale(
  children: { weight_permille: number; node: LayoutNode }[],
): { weight_permille: number; node: LayoutNode }[] {
  const total = sum(children.map((c) => c.weight_permille));
  let used = 0;
  return children.map((child, index) => {
    if (index === children.length - 1) {
      return { ...child, weight_permille: Math.max(1, PERMILLE - used) };
    }
    const weight = Math.max(1, Math.round((child.weight_permille * PERMILLE) / total));
    used += weight;
    return { ...child, weight_permille: weight };
  });
}

function rootOrientation(node: LayoutNode): Orientation {
  if (node.node === "split") {
    return node.data.direction === "row" ? Orientation.HORIZONTAL : Orientation.VERTICAL;
  }
  return Orientation.HORIZONTAL;
}

function writeNode(
  node: LayoutNode,
  panels: Record<string, GroupviewPanelState>,
  groups: Counter,
): SerializedGridObject<GroupPanelViewState> {
  if (node.node === "group") {
    return { type: "leaf", data: writeGroup(node.data, panels, groups), size: PERMILLE };
  }
  return {
    type: "branch",
    data: node.data.children.map((child) => ({
      ...writeNode(child.node, panels, groups),
      size: Math.max(1, child.weight_permille),
    })),
    size: PERMILLE,
  };
}

function writeGroup(
  group: PanelGroup,
  panels: Record<string, GroupviewPanelState>,
  groups: Counter,
): GroupPanelViewState {
  for (const panel of group.panels) {
    panels[panel.id] = {
      id: panel.id,
      contentComponent: panel.kind,
      title: panel.title ?? panelTitle(panel.kind),
      // Panels are never detached from the DOM: the waterfall's WebGL context, the map's camera
      // and every scroll position would otherwise reset each time a tab was switched, and an
      // element measured while detached reports 0×0.
      renderer: "always",
    };
  }
  const views = group.panels.map((panel) => panel.id);
  const active = group.active ?? views[0];
  return {
    id: `group-${groups.next()}`,
    views,
    ...(active === undefined ? {} : { activeView: active }),
  };
}

function readNode(
  node: SerializedGridObject<GroupPanelViewState>,
  orientation: Orientation,
  panels: Record<string, GroupviewPanelState>,
): LayoutNode {
  if (node.type === "leaf") {
    return { node: "group", data: readGroup(node.data as GroupPanelViewState, panels) };
  }
  const raw = (node.data as SerializedGridObject<GroupPanelViewState>[]) ?? [];
  const children = raw.map((child) => ({
    weight_permille: Math.max(1, Math.round(child.size ?? 1)),
    node: readNode(child, orthogonal(orientation), panels),
  }));
  // A branch serialized at HORIZONTAL lays its children out side by side: dockview writes each
  // child's width as its size there, and its height under VERTICAL.
  const direction = orientation === Orientation.HORIZONTAL ? "row" : "column";
  return normalize({ node: "split", data: { direction, children } });
}

function readGroup(
  data: GroupPanelViewState,
  panels: Record<string, GroupviewPanelState>,
): PanelGroup {
  const specs: PanelSpec[] = [];
  for (const id of data.views ?? []) {
    const kind = panels[id]?.contentComponent;
    if (kind === undefined || !isPanelKind(kind)) {
      continue;
    }
    const title = panels[id]?.title;
    specs.push({
      id,
      kind,
      ...(title === undefined || title === panelTitle(kind) ? {} : { title }),
    });
  }
  // `active` is omitted when it is the first panel — the model's documented meaning of absent.
  // Without this the first write after a restore would differ from what was restored, and the
  // dock would persist a layout nobody rearranged.
  const active = data.activeView;
  const canonical =
    active !== undefined && active !== specs[0]?.id && specs.some((p) => p.id === active);
  return { panels: specs, ...(canonical ? { active } : {}) };
}

function orthogonal(orientation: Orientation): Orientation {
  return orientation === Orientation.HORIZONTAL ? Orientation.VERTICAL : Orientation.HORIZONTAL;
}

function isPanelKind(value: string): value is PanelKind {
  return (PANEL_KINDS as readonly string[]).includes(value);
}

function emptyGroup(): LayoutNode {
  return { node: "group", data: { panels: [] } };
}

function clampFraction(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.min(0.98, value)) : 0;
}

function sum(values: number[]): number {
  return values.reduce((acc, value) => acc + value, 0) || 1;
}

/** Group ids are dockview's, not ours — they never leave the dock, so a counter is enough. */
class Counter {
  private value = 0;

  next(): number {
    this.value += 1;
    return this.value;
  }
}
