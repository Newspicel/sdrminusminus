import { ARRAY_LIMITS, LIGHT_SPEED_M_S } from "../../lib/limits";
import type { ArrayElement, ArrayGeometry, GeometryKind } from "../../lib/types";

export const MIN_ELEMENTS = ARRAY_LIMITS.lanes.min;
export const MAX_EXTENT_M = ARRAY_LIMITS.extent_m;
export const OVERLAP_M = 0.005;
export const FALLBACK_SPACING_M = 0.5;
export const LINE_AXIS_DEG = 90;
const STEPS_PER_M = 1_000;
export const POSITION_STEP_M = 1 / STEPS_PER_M;

const PREVIEW_FILL = 0.36;
const SHAPE_TOLERANCE = 1e-3;

export interface ElementXyz {
  x: number;
  y: number;
  z: number;
}

export interface PreviewPoint {
  x: number;
  y: number;
  lane: number;
}

function radians(deg: number): number {
  return (deg * Math.PI) / 180;
}

export function elementCount(lanes: number): number {
  return Math.max(lanes, MIN_ELEMENTS);
}

export function elementPositions(geometry: ArrayGeometry, lanes: number): ElementXyz[] {
  switch (geometry.kind) {
    case "uca": {
      const count = elementCount(lanes);
      const sense = geometry.winding === "counter_clockwise" ? -1 : 1;
      const first = geometry.first_deg ?? 0;
      return Array.from({ length: count }, (_, lane) => {
        const angle = radians(first + (sense * 360 * lane) / count);
        return {
          x: geometry.radius_m * Math.sin(angle),
          y: geometry.radius_m * Math.cos(angle),
          z: 0,
        };
      });
    }
    case "ula": {
      const count = elementCount(lanes);
      const axis = radians(geometry.axis_deg);
      return Array.from({ length: count }, (_, lane) => {
        const offset = (lane - (count - 1) / 2) * geometry.spacing_m;
        return { x: offset * Math.sin(axis), y: offset * Math.cos(axis), z: 0 };
      });
    }
    case "explicit":
      return geometry.positions
        .slice(0, lanes)
        .map((element) => ({ x: element.x_m, y: element.y_m, z: element.z_m ?? 0 }));
  }
}

function distance(a: ElementXyz, b: ElementXyz): number {
  return Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
}

function smallestGap(positions: readonly ElementXyz[]): number | null {
  let smallest: number | null = null;
  for (let a = 0; a < positions.length; a++) {
    for (let b = a + 1; b < positions.length; b++) {
      const first = positions[a];
      const second = positions[b];
      if (first !== undefined && second !== undefined) {
        const gap = distance(first, second);
        smallest = smallest === null ? gap : Math.min(smallest, gap);
      }
    }
  }
  return smallest;
}

export function adjacentSpacingM(geometry: ArrayGeometry, lanes: number): number | null {
  switch (geometry.kind) {
    case "uca":
      return 2 * geometry.radius_m * Math.sin(Math.PI / elementCount(lanes));
    case "ula":
      return geometry.spacing_m;
    case "explicit":
      return smallestGap(elementPositions(geometry, lanes));
  }
}

export function unambiguousHz(geometry: ArrayGeometry, lanes: number): number | null {
  const spacing = adjacentSpacingM(geometry, lanes);
  return spacing === null || !(spacing > 0) ? null : LIGHT_SPEED_M_S / (2 * spacing);
}

function toMillimetre(value: number): number {
  return Math.round(value * STEPS_PER_M) / STEPS_PER_M + 0;
}

function roundedPosition(element: ElementXyz): ArrayElement {
  return {
    x_m: toMillimetre(element.x),
    y_m: toMillimetre(element.y),
    z_m: toMillimetre(element.z),
  };
}

export function convertGeometry(
  to: GeometryKind,
  from: ArrayGeometry,
  lanes: number,
): ArrayGeometry {
  if (to === from.kind) {
    return from;
  }
  const measured = adjacentSpacingM(from, lanes);
  const spacing = measured !== null && measured > 0 ? measured : FALLBACK_SPACING_M;
  switch (to) {
    case "uca":
      return {
        kind: "uca",
        radius_m: spacing / (2 * Math.sin(Math.PI / elementCount(lanes))),
        first_deg: 0,
        winding: "clockwise",
      };
    case "ula":
      return { kind: "ula", spacing_m: spacing, axis_deg: LINE_AXIS_DEG };
    case "explicit":
      return {
        kind: "explicit",
        positions: elementPositions(from, lanes).map(roundedPosition),
      };
  }
}

export type ElementAxis = "x_m" | "y_m" | "z_m";

export function paddedPositions(positions: readonly ArrayElement[], lanes: number): ArrayElement[] {
  return Array.from(
    { length: Math.max(lanes, positions.length) },
    (_, row) => positions[row] ?? { x_m: 0, y_m: 0, z_m: 0 },
  );
}

export function movedElement(
  positions: readonly ArrayElement[],
  row: number,
  axis: ElementAxis,
  value: number,
): ArrayElement[] {
  return positions.map((element, index) =>
    index === row ? { ...element, z_m: element.z_m ?? 0, [axis]: value } : element,
  );
}

function asXyz(element: ArrayElement): ElementXyz {
  return { x: element.x_m, y: element.y_m, z: element.z_m ?? 0 };
}

export function overlappingRows(
  positions: readonly ArrayElement[],
  lanes: number,
  toleranceM = OVERLAP_M,
): number[] {
  const wired = positions.slice(0, lanes).map(asXyz);
  return wired.flatMap((row, index) =>
    wired.slice(0, index).some((earlier) => distance(earlier, row) < toleranceM) ? [index] : [],
  );
}

interface Span {
  from: ElementXyz;
  unit: ElementXyz;
  length: number;
}

function spanOf(from: ElementXyz, to: ElementXyz): Span {
  const length = distance(from, to);
  return {
    from,
    unit: { x: (to.x - from.x) / length, y: (to.y - from.y) / length, z: (to.z - from.z) / length },
    length,
  };
}

function widestSpan(positions: readonly ElementXyz[]): Span | null {
  let widest: Span | null = null;
  for (let a = 0; a < positions.length; a++) {
    for (let b = a + 1; b < positions.length; b++) {
      const from = positions[a];
      const to = positions[b];
      if (from !== undefined && to !== undefined && distance(from, to) > (widest?.length ?? 0)) {
        widest = spanOf(from, to);
      }
    }
  }
  return widest;
}

function along(span: Span, point: ElementXyz): number {
  return (
    (point.x - span.from.x) * span.unit.x +
    (point.y - span.from.y) * span.unit.y +
    (point.z - span.from.z) * span.unit.z
  );
}

function offLine(span: Span, point: ElementXyz): number {
  const run = along(span, point);
  return Math.hypot(
    point.x - span.from.x - run * span.unit.x,
    point.y - span.from.y - run * span.unit.y,
    point.z - span.from.z - run * span.unit.z,
  );
}

function lineSpan(positions: readonly ElementXyz[]): Span | null {
  const span = widestSpan(positions);
  if (span === null) {
    return null;
  }
  const tolerance = SHAPE_TOLERANCE * span.length;
  return positions.every((point) => offLine(span, point) <= tolerance) ? span : null;
}

function level(positions: readonly ElementXyz[], tolerance: number): boolean {
  const meanZ = positions.reduce((sum, point) => sum + point.z, 0) / positions.length;
  return positions.every((point) => Math.abs(point.z - meanZ) <= tolerance);
}

function levelLine(positions: readonly ElementXyz[]): Span | null {
  const span = lineSpan(positions);
  return span !== null && level(positions, SHAPE_TOLERANCE * span.length) ? span : null;
}

export function isCollinear(geometry: ArrayGeometry, lanes: number): boolean {
  return geometry.kind === "ula" || lineSpan(elementPositions(geometry, lanes)) !== null;
}

export function isLevelLine(geometry: ArrayGeometry, lanes: number): boolean {
  return geometry.kind === "ula" || levelLine(elementPositions(geometry, lanes)) !== null;
}

function evenlySpaced(positions: readonly ElementXyz[]): boolean {
  const span = levelLine(positions);
  if (span === null) {
    return false;
  }
  const runs = positions.map((point) => along(span, point)).toSorted((a, b) => a - b);
  const step = span.length / (runs.length - 1);
  const tolerance = SHAPE_TOLERANCE * span.length;
  return runs.every(
    (run, index) => index === 0 || Math.abs(run - (runs[index - 1] ?? run) - step) <= tolerance,
  );
}

export function allowsStructured(geometry: ArrayGeometry, lanes: number): boolean {
  return (
    geometry.kind === "uca" ||
    geometry.kind === "ula" ||
    evenlySpaced(elementPositions(geometry, lanes))
  );
}

export function previewPoints(geometry: ArrayGeometry, lanes: number, box: number): PreviewPoint[] {
  const positions = elementPositions(geometry, lanes);
  const reach = Math.max(
    0,
    ...positions.map((point) => Math.max(Math.abs(point.x), Math.abs(point.y))),
  );
  const scale = reach > 0 ? (PREVIEW_FILL * box) / reach : 0;
  const centre = box / 2;
  return positions.map((point, index) => ({
    x: centre + point.x * scale,
    y: centre - point.y * scale,
    lane: index + 1,
  }));
}
