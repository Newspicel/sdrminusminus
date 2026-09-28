import type { PolarimeterParams, PolarimeterReading } from "../../lib/types";

type Hand = NonNullable<PolarimeterReading["hand"]>;

export const ELLIPSE_POINTS = 64;
export const PICK_TWO = "Pick two lanes";
export const LINEAR_DEG = 2;
export const ARROW_STEP = 0.35;
export const ARROW_SIZE = 5;

const HAND_TEXT: Readonly<Record<Hand, string>> = { right: "RH", left: "LH", linear: "Lin" };

export function handText(hand: Hand | undefined): string {
  return HAND_TEXT[hand ?? "linear"];
}

function toRadians(deg: number): number {
  return (deg * Math.PI) / 180;
}

export function ellipsePoint(
  t: number,
  angleDeg: number,
  ellipticityDeg: number,
  radius: number,
  centre: number,
): { x: number; y: number } {
  const psi = toRadians(angleDeg);
  const chi = toRadians(ellipticityDeg);
  const along = Math.cos(chi) * Math.cos(t);
  const across = Math.sin(chi) * Math.sin(t);
  const x = along * Math.cos(psi) - across * Math.sin(psi);
  const y = along * Math.sin(psi) + across * Math.cos(psi);
  return { x: centre + radius * x, y: centre - radius * y };
}

export function ellipsePath(
  angleDeg: number,
  ellipticityDeg: number,
  radius: number,
  centre: number,
  points = ELLIPSE_POINTS,
): string {
  const steps = Math.max(3, points);
  const parts: string[] = [];
  for (let step = 0; step < steps; step++) {
    const { x, y } = ellipsePoint(
      (step / steps) * Math.PI * 2,
      angleDeg,
      ellipticityDeg,
      radius,
      centre,
    );
    parts.push(`${step === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`);
  }
  return `${parts.join("")}Z`;
}

export function senseStep(v: number, ellipticityDeg: number): number {
  const clockwise = v > 0;
  const forwardTurnsLeft = ellipticityDeg > 0;
  return clockwise === forwardTurnsLeft ? -ARROW_STEP : ARROW_STEP;
}

export interface SenseArrow {
  arc: string;
  head: string;
}

function pointText(point: { x: number; y: number }): string {
  return `${point.x.toFixed(1)} ${point.y.toFixed(1)}`;
}

export function senseArrow(
  angleDeg: number,
  ellipticityDeg: number,
  v: number,
  radius: number,
  centre: number,
): SenseArrow | null {
  if (Math.abs(ellipticityDeg) <= LINEAR_DEG) {
    return null;
  }
  const step = senseStep(v, ellipticityDeg);
  const at = (t: number) => ellipsePoint(t, angleDeg, ellipticityDeg, radius, centre);
  const start = at(0);
  const tip = at(step);
  const back = at(step * 0.8);
  const length = Math.hypot(tip.x - back.x, tip.y - back.y);
  if (length === 0) {
    return null;
  }
  const dx = (tip.x - back.x) / length;
  const dy = (tip.y - back.y) / length;
  const base = { x: tip.x - dx * ARROW_SIZE, y: tip.y - dy * ARROW_SIZE };
  const half = ARROW_SIZE * 0.6;
  const left = { x: base.x - dy * half, y: base.y + dx * half };
  const right = { x: base.x + dy * half, y: base.y - dx * half };
  return {
    arc: `M${pointText(start)}L${pointText(back)}`,
    head: `M${pointText(left)}L${pointText(tip)}L${pointText(right)}Z`,
  };
}

export function laneEdit(
  settings: PolarimeterParams,
  which: "h_lane" | "v_lane",
  lane: number,
): Partial<PolarimeterParams> | null {
  const other = which === "h_lane" ? settings.v_lane : settings.h_lane;
  return lane === other ? null : { [which]: lane };
}

export function stokesText(value: number): string {
  return value.toFixed(2);
}

export function percentText(fraction: number): string {
  return `${Math.round(Math.min(1, Math.max(0, fraction)) * 100)}%`;
}
