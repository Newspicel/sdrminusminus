import type { SatellitePass, Transmitter } from "../../lib/types";

const COMPASS = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"] as const;
export const STALE_ELEMENTS_DAYS = 7;
export const SATELLITE_RANGE = { min: 1e6, max: 12e9 };

export function compass(azimuthDeg: number): string {
  const index = Math.round((((azimuthDeg % 360) + 360) % 360) / 45) % COMPASS.length;
  return COMPASS[index] ?? "N";
}

export function formatDoppler(hz: number): string {
  const sign = hz > 0 ? "+" : hz < 0 ? "−" : "";
  const magnitude = Math.abs(hz);
  return magnitude >= 1_000
    ? `${sign}${(magnitude / 1_000).toFixed(2)} kHz`
    : `${sign}${magnitude.toFixed(0)} Hz`;
}

export function formatSpan(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  const hours = Math.floor(whole / 3_600);
  const minutes = Math.floor((whole % 3_600) / 60);
  const rest = whole % 60;
  if (hours > 0) {
    return `${hours} h ${String(minutes).padStart(2, "0")} min`;
  }
  return `${minutes}:${String(rest).padStart(2, "0")}`;
}

export function passLine(pass: SatellitePass | null | undefined, nowS: number): string {
  if (pass == null) {
    return "none in 48 h";
  }
  const peak = `${pass.max_elevation_deg.toFixed(0)}°`;
  if (pass.aos != null && pass.aos > nowS) {
    return `in ${formatSpan(pass.aos - nowS)}, up to ${peak}`;
  }
  if (pass.los != null) {
    return `sets in ${formatSpan(pass.los - nowS)}, up to ${peak}`;
  }
  return "always up";
}

export function pastedElements(text: string): string | null {
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.trim() !== "");
  const first = lines.findIndex((line) => line.startsWith("1 "));
  if (first < 0 || !(lines[first + 1]?.startsWith("2 ") ?? false)) {
    return null;
  }
  return lines.slice(Math.max(0, first - 1), first + 2).join("\n");
}

export function transmitterLabel(transmitter: Transmitter): string {
  const mhz =
    transmitter.downlink_hz == null ? "" : ` ${(transmitter.downlink_hz / 1e6).toFixed(3)}`;
  const mode = transmitter.mode == null ? "" : ` ${transmitter.mode}`;
  return `${transmitter.description}${mode}${mhz}${transmitter.alive ? "" : " (off)"}`;
}

export function shownSignals(
  transmitters: readonly Transmitter[],
  chosen: string | null | undefined,
): Transmitter[] {
  return transmitters.filter(
    (transmitter) =>
      transmitter.downlink_hz != null && (transmitter.alive || transmitter.id === chosen),
  );
}
