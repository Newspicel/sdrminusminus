import type {
  AgcSetting,
  BandwidthSetting,
  Capabilities,
  DeviceSettings,
  GainKind,
  GainStage,
  Range,
} from "../lib/types";
import { settingLabel } from "./settingLabel";

const GAIN_LABEL: Record<GainKind, string> = {
  lna: "LNA",
  mixer: "Mixer",
  vga: "VGA",
  if: "IF",
  rf: "RF",
  tuner: "Tuner",
  amp: "Amp",
  attenuator: "Attenuator",
  tx: "TX",
  other: "",
};

export function gainLabel(stage: Pick<GainStage, "kind" | "name">): string {
  return GAIN_LABEL[stage.kind] || settingLabel(stage.name);
}

export function gainUnit(stage: Pick<GainStage, "unit">): string {
  return (stage.unit ?? "db") === "index" ? "" : "dB";
}

export function formatGain(stage: Pick<GainStage, "unit">, value: number): string {
  return (stage.unit ?? "db") === "index" ? String(Math.round(value)) : value.toFixed(1);
}

export function isSwitch(stage: Pick<GainStage, "kind">): boolean {
  return stage.kind === "amp";
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

export function agcOffered(caps: Pick<Capabilities, "agc">): boolean {
  return (caps.agc?.kind ?? "none") !== "none";
}

export function agcState(
  caps: Pick<Capabilities, "agc">,
  settings: Pick<DeviceSettings, "agc">,
): AgcSetting {
  const modes = caps.agc?.kind === "modes" ? caps.agc.options : [];
  const firstMode = modes[0]?.value;
  const reported = settings.agc;
  const mode = reported?.mode ?? firstMode;
  return {
    on: agcOffered(caps) && (reported?.on ?? false),
    ...(mode == null ? {} : { mode }),
  };
}

export function automaticGainIsOn(
  caps: Pick<Capabilities, "agc">,
  settings: Pick<DeviceSettings, "agc">,
): boolean {
  return agcState(caps, settings).on;
}

export function hasFilter(
  caps: Pick<Capabilities, "bandwidths" | "bandwidth_ranges" | "bandwidth_auto">,
): boolean {
  return (
    caps.bandwidth_auto === true ||
    caps.bandwidths.length > 0 ||
    (caps.bandwidth_ranges?.length ?? 0) > 0
  );
}

export function filterIsAuto(settings: Pick<DeviceSettings, "bandwidth">): boolean {
  return settings.bandwidth?.kind === "auto";
}

export function filterHz(
  caps: Pick<Capabilities, "bandwidths" | "bandwidth_ranges">,
  settings: Pick<DeviceSettings, "bandwidth" | "sample_rate">,
): number {
  const bandwidth = settings.bandwidth;
  if (bandwidth?.kind === "manual") return bandwidth.hz;
  const rate = settings.sample_rate ?? 0;
  const menu = caps.bandwidths.toSorted((a, b) => a - b);
  const wideEnough = menu.find((hz) => hz >= rate);
  if (wideEnough != null) return wideEnough;
  const widest = menu.at(-1);
  if (widest != null) return widest;
  return snapToRanges(caps.bandwidth_ranges, rate);
}

export function manualFilter(hz: number): BandwidthSetting {
  return { kind: "manual", hz };
}

export const AUTO_FILTER: BandwidthSetting = { kind: "auto" };
