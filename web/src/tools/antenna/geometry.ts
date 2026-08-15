import type { AntennaGeometry, AntennaPoint, AntennaSegmentRole } from "../../lib/types";
import { formatLength, type LengthUnit } from "./antenna";

export type Axis = "x" | "y" | "z";

const COORDINATE: Record<Axis, keyof AntennaPoint> = { x: "x_m", y: "y_m", z: "z_m" };

export interface Extent {
  min: number;
  max: number;
  size: number;
}

export type Bounds = Record<Axis, Extent>;

export interface Point2 {
  x: number;
  y: number;
}

/** Where the eye is: a turn about the vertical, then a tilt. Degrees. */
export interface Angles {
  yaw: number;
  pitch: number;
}

export const ISOMETRIC: Angles = { yaw: -32, pitch: 22 };

export const MAX_PITCH = 88;

export function boundsOf(geometry: AntennaGeometry): Bounds {
  return boundsOfPoints([
    geometry.feed,
    ...geometry.segments.flatMap((segment) => [segment.from, segment.to]),
  ]);
}

/** How big the antenna is, which is not how much room the drawing takes: a length of coax
 * hanging off the feedpoint is cut to a number, not part of the thing's size. */
export function structureBounds(geometry: AntennaGeometry): Bounds {
  const structure = geometry.segments.filter((segment) => segment.role !== "feedline");
  return boundsOfPoints(structure.flatMap((segment) => [segment.from, segment.to]));
}

function boundsOfPoints(points: readonly AntennaPoint[]): Bounds {
  return { x: extentOf(points, "x"), y: extentOf(points, "y"), z: extentOf(points, "z") };
}

function extentOf(points: readonly AntennaPoint[], axis: Axis): Extent {
  const values = points.map((point) => point[COORDINATE[axis]]);
  const min = values.length === 0 ? 0 : Math.min(...values);
  const max = values.length === 0 ? 0 : Math.max(...values);
  return { min, max, size: max - min };
}

/** The two views a flat drawing can be: the plane the antenna actually lives in. */
export interface PlanView {
  label: string;
  angles: Angles;
  horizontal: Axis;
  vertical: Axis;
}

const FRONT: PlanView = {
  label: "Front view",
  angles: { yaw: 0, pitch: 0 },
  horizontal: "x",
  vertical: "y",
};
const TOP: PlanView = {
  label: "Top view",
  angles: { yaw: 0, pitch: -90 },
  horizontal: "x",
  vertical: "z",
};
const SIDE: PlanView = {
  label: "Side view",
  angles: { yaw: 90, pitch: 0 },
  horizontal: "z",
  vertical: "y",
};

/**
 * Look down the axis the antenna is thinnest along, so nothing important collapses to a point:
 * a dipole and a quad are drawn face on, a Yagi from above its boom.
 */
export function planView(bounds: Bounds): PlanView {
  if (bounds.z.size <= bounds.x.size && bounds.z.size <= bounds.y.size) {
    return FRONT;
  }
  return bounds.y.size <= bounds.x.size ? TOP : SIDE;
}

/** Metres to a flat drawing, with the screen's y already pointing down. */
export function project(point: AntennaPoint, angles: Angles): Point2 {
  const yaw = (angles.yaw * Math.PI) / 180;
  const pitch = (angles.pitch * Math.PI) / 180;
  const across = point.x_m * Math.cos(yaw) + point.z_m * Math.sin(yaw);
  const depth = point.z_m * Math.cos(yaw) - point.x_m * Math.sin(yaw);
  const up = point.y_m * Math.cos(pitch) - depth * Math.sin(pitch);
  return { x: across, y: -up };
}

export interface Viewport {
  width: number;
  height: number;
  padding: number;
}

/** Drawing units per metre, plus the offset that centres the result in the viewport. */
export interface Fit {
  scale: number;
  offsetX: number;
  offsetY: number;
}

export function fitTo(points: readonly Point2[], viewport: Viewport): Fit {
  const centred: Fit = { scale: 1, offsetX: viewport.width / 2, offsetY: viewport.height / 2 };
  if (points.length === 0) {
    return centred;
  }
  const xs = points.map((point) => point.x);
  const ys = points.map((point) => point.y);
  const spanX = Math.max(...xs) - Math.min(...xs);
  const spanY = Math.max(...ys) - Math.min(...ys);
  const room = Math.max(1, viewport.width - 2 * viewport.padding);
  const height = Math.max(1, viewport.height - 2 * viewport.padding);
  const scale = Math.min(
    spanX > 0 ? room / spanX : Number.POSITIVE_INFINITY,
    spanY > 0 ? height / spanY : Number.POSITIVE_INFINITY,
  );
  if (!Number.isFinite(scale) || scale <= 0) {
    return centred;
  }
  const centreX = (Math.max(...xs) + Math.min(...xs)) / 2;
  const centreY = (Math.max(...ys) + Math.min(...ys)) / 2;
  return {
    scale,
    offsetX: viewport.width / 2 - centreX * scale,
    offsetY: viewport.height / 2 - centreY * scale,
  };
}

export function place(point: Point2, fit: Fit): Point2 {
  return { x: point.x * fit.scale + fit.offsetX, y: point.y * fit.scale + fit.offsetY };
}

export interface ScaleBar {
  meters: number;
  pixels: number;
  label: string;
}

const METERS_PER_FOOT = 0.3048;

/** The longest round length that still fits in `maxPixels`, so the drawing carries its own ruler. */
export function scaleBar(pixelsPerMeter: number, maxPixels: number, unit: LengthUnit): ScaleBar {
  const room = maxPixels / pixelsPerMeter;
  if (!Number.isFinite(room) || room <= 0) {
    return { meters: 0, pixels: 0, label: "—" };
  }
  const meters =
    unit === "ft" ? roundDown(room / METERS_PER_FOOT) * METERS_PER_FOOT : roundDown(room);
  return { meters, pixels: meters * pixelsPerMeter, label: formatLength(meters, unit) };
}

/** Down to the nearest 1, 2 or 5 of whatever decade the value sits in. */
function roundDown(value: number): number {
  const decade = 10 ** Math.floor(Math.log10(value));
  const steps = [5, 2, 1];
  return (steps.find((step) => value >= step * decade) ?? 1) * decade;
}

/** Colour carries the job a piece does; width and dash carry it again for anyone who cannot
 * separate the hues. */
export const ROLE_STYLE: Record<
  AntennaSegmentRole,
  { stroke: string; width: number; dash?: string }
> = {
  driven: { stroke: "stroke-accent", width: 3 },
  parasitic: { stroke: "stroke-ink", width: 2.5 },
  radial: { stroke: "stroke-ink-dim", width: 1.5 },
  matching: { stroke: "stroke-ok", width: 2, dash: "5 3" },
  structure: { stroke: "stroke-line-strong", width: 4 },
  feedline: { stroke: "stroke-ink-faint", width: 2, dash: "2 4" },
};

export const ROLE_LABEL: Record<AntennaSegmentRole, string> = {
  driven: "Driven",
  parasitic: "Parasitic",
  radial: "Radial",
  matching: "Matching",
  structure: "Structure",
  feedline: "Feedline",
};

/** The roles actually present, in a fixed order, so the legend explains this drawing and no
 * other. */
export function rolesIn(geometry: AntennaGeometry): AntennaSegmentRole[] {
  const order: AntennaSegmentRole[] = [
    "driven",
    "parasitic",
    "radial",
    "matching",
    "structure",
    "feedline",
  ];
  return order.filter((role) => geometry.segments.some((segment) => segment.role === role));
}
