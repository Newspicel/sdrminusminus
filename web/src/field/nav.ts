import type { DfGuidance, ManeuverKind, Route, RoutePoint } from "../lib/types";

const EARTH_RADIUS_M = 6_371_000;

/// How far off the drawn route counts as having left it. Wide enough that a GPS wobble or a
/// parallel service road is not a re-route, tight enough that a wrong turn is.
export const OFF_ROUTE_M = 50;
/// How far the nav target has to move before the route is worth asking for again.
export const RETARGET_M = 250;
/// Inside this the next maneuver is announced, once.
export const ANNOUNCE_M = 200;

export function toRadians(degrees: number): number {
  return (degrees * Math.PI) / 180;
}

export function distanceM(from: RoutePoint, to: RoutePoint): number {
  const phi1 = toRadians(from.lat);
  const phi2 = toRadians(to.lat);
  const dphi = phi2 - phi1;
  const dlambda = toRadians(to.lon - from.lon);
  const a = Math.sin(dphi / 2) ** 2 + Math.cos(phi1) * Math.cos(phi2) * Math.sin(dlambda / 2) ** 2;
  return 2 * EARTH_RADIUS_M * Math.asin(Math.min(1, Math.sqrt(a)));
}

/// How far a point is from the drawn line, measured against the nearest segment rather than the
/// nearest vertex — a long straight leg has few vertices and a driver can be far from all of them
/// while still on the road.
export function distanceToRouteM(route: Route, at: RoutePoint): number {
  if (route.polyline.length === 0) {
    return Number.POSITIVE_INFINITY;
  }
  if (route.polyline.length === 1) {
    return distanceM(route.polyline[0] as RoutePoint, at);
  }
  let best = Number.POSITIVE_INFINITY;
  const scale = Math.cos(toRadians(at.lat));
  for (let index = 1; index < route.polyline.length; index++) {
    const a = route.polyline[index - 1] as RoutePoint;
    const b = route.polyline[index] as RoutePoint;
    const ax = (a.lon - at.lon) * scale;
    const ay = a.lat - at.lat;
    const bx = (b.lon - at.lon) * scale;
    const by = b.lat - at.lat;
    const dx = bx - ax;
    const dy = by - ay;
    const length = dx * dx + dy * dy;
    const t = length === 0 ? 0 : Math.min(1, Math.max(0, -(ax * dx + ay * dy) / length));
    const x = ax + t * dx;
    const y = ay + t * dy;
    best = Math.min(best, Math.hypot(x, y));
  }
  return (best * Math.PI * EARTH_RADIUS_M) / 180;
}

export type ReroutePrompt = "none" | "no-route" | "off-route" | "target-moved" | "mode-changed";

export interface RouteState {
  route: Route | null;
  target: RoutePoint | null;
  mode: DfGuidance["mode"] | null;
}

/// Whether the route has to be asked for again.
///
/// Only ever a change worth a request: leaving the road, the target moving, or the guidance
/// flipping from crossing to approaching. Never a timer — a free tier is a handful of requests a
/// minute, and a timer would spend them on nothing.
export function reroutePrompt(
  state: RouteState,
  guidance: DfGuidance | null,
  at: RoutePoint | null,
): ReroutePrompt {
  if (guidance === null) {
    return "none";
  }
  if (state.route === null || state.target === null) {
    return "no-route";
  }
  if (state.mode !== guidance.mode) {
    return "mode-changed";
  }
  if (distanceM(state.target, guidance.nav_target) > RETARGET_M) {
    return "target-moved";
  }
  if (at !== null && distanceToRouteM(state.route, at) > OFF_ROUTE_M) {
    return "off-route";
  }
  return "none";
}

/// The maneuver the driver is coming up to, and how far away it is.
export function nextManeuver(
  route: Route | null,
  at: RoutePoint | null,
): { instruction: string; kind: ManeuverKind; distanceM: number } | null {
  if (route === null || at === null || route.maneuvers.length === 0) {
    return null;
  }
  let best = null;
  for (const maneuver of route.maneuvers) {
    const away = distanceM(at, maneuver.at);
    if (best === null || away < best.distanceM) {
      best = { instruction: maneuver.instruction, kind: maneuver.kind, distanceM: away };
    }
  }
  return best;
}

export function shouldAnnounce(
  next: { instruction: string; distanceM: number } | null,
  announced: string | null,
): boolean {
  return next !== null && next.distanceM <= ANNOUNCE_M && next.instruction !== announced;
}

/// A deep link that hands the leg to whatever the phone navigates with.
///
/// A browser cannot re-fire this on its own: the page is in the background while the native app
/// drives, so the button has to be pressed again after every retarget. That is why it is the
/// fallback and the in-page route is the automatic path.
export function handoffUrl(target: RoutePoint, platform: string): string {
  const coordinates = `${target.lat},${target.lon}`;
  if (/iphone|ipad|ipod|mac/i.test(platform)) {
    return `maps://?daddr=${coordinates}&dirflg=d`;
  }
  if (/android/i.test(platform)) {
    return `google.navigation:q=${coordinates}`;
  }
  return `https://www.openstreetmap.org/directions?to=${coordinates}`;
}

export function formatDistance(metres: number): string {
  if (!Number.isFinite(metres)) {
    return "—";
  }
  return metres < 1_000 ? `${Math.round(metres / 10) * 10} m` : `${(metres / 1_000).toFixed(1)} km`;
}

/// What the compass needle should read: the bearing to steer minus where the vehicle is pointing,
/// so straight up on the rose is always "carry on".
export function relativeHeading(headingDeg: number, trackDeg: number | null | undefined): number {
  return (headingDeg - (trackDeg ?? 0) + 360) % 360;
}
