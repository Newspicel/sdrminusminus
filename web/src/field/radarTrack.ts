import type { RadarDetection } from "../lib/types";

/// One followed echo, in the terms a driver watching the screen can read: how much further it
/// travelled than the direct path, and which way it is moving.
export function trackLabel(hit: RadarDetection): string {
  const closing = hit.doppler_hz > 0 ? "closing" : "opening";
  const name = hit.track_id == null ? "—" : String(hit.track_id).padStart(2, "0");
  return `${name}  ${hit.range_km.toFixed(1)} km  ${Math.abs(hit.doppler_hz).toFixed(0)} Hz ${closing}`;
}
