import type { Capabilities, DeviceSettings, GainStage, Range } from "../lib/types";

export function isSwitch(stage: GainStage): boolean {
  const values = stage.values ?? [];
  if (values.length > 0) return values.length === 2;
  const { min, max, step } = stage.range;
  return step != null && step > 0 && max - min === step;
}

export function stageSettings(stage: GainStage): number[] {
  const values = stage.values ?? [];
  if (values.length > 0) return values.toSorted((a, b) => a - b);
  const { min, max, step } = stage.range;
  if (step == null || step <= 0) return [];
  const settings: number[] = [];
  for (let value = min; value <= max + step / 2; value += step) {
    settings.push(Math.min(value, max));
  }
  return settings;
}

export function snapToStage(stage: GainStage, db: number): number {
  const clamped = Math.min(Math.max(db, stage.range.min), stage.range.max);
  const settings = stageSettings(stage);
  if (settings.length === 0) return clamped;
  let best = settings[0] as number;
  for (const setting of settings) {
    if (Math.abs(setting - clamped) < Math.abs(best - clamped)) best = setting;
  }
  return best;
}

export function settingIndex(settings: number[], db: number): number {
  let best = 0;
  for (let index = 0; index < settings.length; index += 1) {
    const candidate = settings[index] as number;
    const current = settings[best] as number;
    if (Math.abs(candidate - db) < Math.abs(current - db)) best = index;
  }
  return best;
}

const SLIDER_STEPS = 100;

export function fitsSlider(range: Range): boolean {
  const { min, max, step } = range;
  if (step == null || step <= 0 || max <= min) return false;
  return (max - min) / step <= SLIDER_STEPS;
}

export function spanOf(ranges: Range[] | undefined): Range | undefined {
  if (ranges == null || ranges.length === 0) return undefined;
  const min = Math.min(...ranges.map((range) => range.min));
  const max = Math.max(...ranges.map((range) => range.max));
  const steps = ranges.map((range) => range.step).filter((step): step is number => step != null);
  return { min, max, step: steps.length === ranges.length ? Math.min(...steps) : undefined };
}

/**
 * Windows can have gaps in them — the RTL2832U aliases between 300 kHz and 900 kHz — so a control
 * bounded by the outer span alone would offer rates the radio refuses.
 */
export function snapToRanges(ranges: Range[] | undefined, value: number): number {
  if (ranges == null || ranges.length === 0) return value;
  let best = Math.min(Math.max(value, ranges[0]!.min), ranges[0]!.max);
  for (const range of ranges) {
    const held = Math.min(Math.max(value, range.min), range.max);
    if (Math.abs(held - value) < Math.abs(best - value)) best = held;
  }
  return best;
}

export function hasDcArtifact(caps: Pick<Capabilities, "dc_artifact">): boolean {
  return (caps.dc_artifact ?? "operator") !== "none";
}

export function dcBlockOn(
  caps: Pick<Capabilities, "dc_artifact">,
  settings: Pick<DeviceSettings, "dc_block">,
): boolean {
  return settings.dc_block ?? caps.dc_artifact === "managed";
}

export const AGC_SETTING = "gain_mode";

/**
 * Whether the radio is choosing its own gain. A stage written while it is only lasts until the
 * next correction, so the controls read out what the radio picked rather than offering a value
 * that will not survive.
 */
export function automaticGainIsOn(
  caps: Pick<Capabilities, "extra">,
  settings: Pick<DeviceSettings, "extra">,
): boolean {
  if (!(caps.extra ?? []).some((setting) => setting.name === AGC_SETTING)) return false;
  return settings.extra?.find((value) => value.name === AGC_SETTING)?.value === true;
}
