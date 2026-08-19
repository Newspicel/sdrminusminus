import { describe, expect, it } from "vitest";
import type { DfGuidance, Route } from "../lib/types";
import {
  distanceM,
  distanceToRouteM,
  formatDistance,
  handoffUrl,
  nextManeuver,
  OFF_ROUTE_M,
  RETARGET_M,
  type RouteState,
  relativeHeading,
  reroutePrompt,
  shouldAnnounce,
} from "./nav";

const HOME = { lat: 51.5, lon: 7.0 };

function route(): Route {
  return {
    polyline: [
      { lat: 51.5, lon: 7.0 },
      { lat: 51.51, lon: 7.0 },
      { lat: 51.52, lon: 7.0 },
    ],
    distance_m: 2_200,
    duration_s: 200,
    maneuvers: [
      { at: { lat: 51.5, lon: 7.0 }, kind: "depart", instruction: "Head north", distance_m: 0 },
      { at: { lat: 51.51, lon: 7.0 }, kind: "left", instruction: "Turn left", distance_m: 1_100 },
    ],
  };
}

function guidance(over: Partial<DfGuidance> = {}): DfGuidance {
  return {
    heading_deg: 90,
    mode: "cross",
    distance_m: 1_500,
    nav_target: { lat: 51.52, lon: 7.0, kind: "cross" },
    ...over,
  };
}

function state(over: Partial<RouteState> = {}): RouteState {
  return { route: route(), target: { lat: 51.52, lon: 7.0 }, mode: "cross", ...over };
}

describe("distanceToRouteM", () => {
  it("measures to the nearest segment, not the nearest vertex", () => {
    const halfway = { lat: 51.515, lon: 7.0 };
    expect(distanceToRouteM(route(), halfway)).toBeLessThan(5);
    const beside = { lat: 51.515, lon: 7.002 };
    expect(distanceToRouteM(route(), beside)).toBeGreaterThan(100);
  });

  it("has nothing to measure against an empty route", () => {
    const empty: Route = { polyline: [], distance_m: 0, duration_s: 0, maneuvers: [] };
    expect(distanceToRouteM(empty, HOME)).toBe(Number.POSITIVE_INFINITY);
  });
});

describe("reroutePrompt", () => {
  it("asks for a route when there is none", () => {
    expect(reroutePrompt({ route: null, target: null, mode: null }, guidance(), HOME)).toBe(
      "no-route",
    );
  });

  it("says nothing while the driver is on the route and the target has not moved", () => {
    expect(reroutePrompt(state(), guidance(), { lat: 51.505, lon: 7.0 })).toBe("none");
  });

  it("asks again once the driver leaves the route", () => {
    const off = { lat: 51.505, lon: 7.0 + (OFF_ROUTE_M * 4) / 111_320 };
    expect(reroutePrompt(state(), guidance(), off)).toBe("off-route");
  });

  it("asks again once the target has moved far enough to matter", () => {
    const moved = guidance({
      nav_target: { lat: 51.52 + (RETARGET_M * 4) / 111_320, lon: 7.0, kind: "cross" },
    });
    expect(reroutePrompt(state(), moved, HOME)).toBe("target-moved");
  });

  it("asks again when the guidance flips from crossing to approaching", () => {
    expect(reroutePrompt(state(), guidance({ mode: "approach" }), HOME)).toBe("mode-changed");
  });

  it("asks for nothing at all without guidance", () => {
    expect(reroutePrompt(state(), null, HOME)).toBe("none");
  });
});

describe("nextManeuver", () => {
  it("picks the one the driver is closest to", () => {
    const next = nextManeuver(route(), { lat: 51.5095, lon: 7.0 });
    expect(next?.instruction).toBe("Turn left");
    expect(next?.distanceM).toBeLessThan(200);
    expect(nextManeuver(null, HOME)).toBeNull();
  });
});

describe("shouldAnnounce", () => {
  it("speaks each maneuver once, and only once it is close", () => {
    const far = { instruction: "Turn left", distanceM: 900 };
    const near = { instruction: "Turn left", distanceM: 120 };
    expect(shouldAnnounce(far, null)).toBe(false);
    expect(shouldAnnounce(near, null)).toBe(true);
    expect(shouldAnnounce(near, "Turn left")).toBe(false);
    expect(shouldAnnounce(null, null)).toBe(false);
  });
});

describe("handoffUrl", () => {
  it("uses whatever the phone navigates with", () => {
    const target = { lat: 51.5, lon: 7.0 };
    expect(handoffUrl(target, "iPhone")).toContain("maps://");
    expect(handoffUrl(target, "Android 15")).toContain("google.navigation:");
    expect(handoffUrl(target, "X11; Linux")).toContain("openstreetmap.org");
  });
});

describe("relativeHeading", () => {
  it("turns a compass bearing into where to point on the rose", () => {
    expect(relativeHeading(90, 90)).toBe(0);
    expect(relativeHeading(0, 90)).toBe(270);
    expect(relativeHeading(45, null)).toBe(45);
  });
});

describe("formatDistance", () => {
  it("rounds to something a driver can read at a glance", () => {
    expect(formatDistance(123)).toBe("120 m");
    expect(formatDistance(2_450)).toBe("2.5 km");
    expect(formatDistance(Number.POSITIVE_INFINITY)).toBe("—");
  });
});

describe("distanceM", () => {
  it("measures a leg on the globe", () => {
    expect(distanceM(HOME, { lat: 51.51, lon: 7.0 })).toBeGreaterThan(1_000);
    expect(distanceM(HOME, HOME)).toBe(0);
  });
});
