import type {
  AudioProcessing,
  ChannelDescriptor,
  ChannelParams,
  ChannelSettings,
  NotchSettings,
  ParamLimit,
  Squelch,
} from "../lib/types";

export type ChannelTypeId = ChannelParams["type"];

export type ChannelParamsOf<K extends ChannelTypeId> = Extract<
  ChannelParams,
  { type: K }
>["settings"];

export function mergeChannelSettings(
  current: ChannelSettings,
  edit: Partial<ChannelSettings>,
): ChannelSettings {
  return {
    frequency_hz: edit.frequency_hz ?? current.frequency_hz,
    squelch: edit.squelch ?? current.squelch ?? SQUELCH_OFF,
    params: edit.params ?? current.params,
    blanker: edit.blanker ?? current.blanker ?? {},
  };
}

export const SQUELCH_RANGE_DB = { min: -120, max: 0 } as const;

export const DEFAULT_SQUELCH_DB = -60;

export type SquelchMode = Squelch["mode"];

export const SQUELCH_OFF: Squelch = { mode: "off" };

export function squelchMode(squelch: Squelch | undefined): SquelchMode {
  return squelch?.mode ?? "off";
}

export function squelchLevelDb(squelch: Squelch | undefined): number | null {
  return squelch?.mode === "manual" ? squelch.level_db : null;
}

export function squelchMarginDb(squelch: Squelch | undefined): number | null {
  return squelch?.mode === "auto" ? squelch.margin_db : null;
}

export function squelchAt(mode: SquelchMode, held: { levelDb: number; marginDb: number }): Squelch {
  switch (mode) {
    case "off":
      return SQUELCH_OFF;
    case "manual":
      return { mode: "manual", level_db: held.levelDb };
    case "auto":
      return { mode: "auto", margin_db: held.marginDb };
  }
}

export function nudgedSquelch(squelch: Squelch | undefined, deltaDb: number): Squelch {
  if (squelch?.mode === "auto") {
    const { min, max } = AUDIO_LIMITS.squelchAutoMarginDb;
    return { mode: "auto", margin_db: Math.min(max, Math.max(min, squelch.margin_db + deltaDb)) };
  }
  const level = (squelchLevelDb(squelch) ?? DEFAULT_SQUELCH_DB) + deltaDb;
  return {
    mode: "manual",
    level_db: Math.min(SQUELCH_RANGE_DB.max, Math.max(SQUELCH_RANGE_DB.min, level)),
  };
}

export interface NumberLimit {
  min?: number;
  max?: number;
  step?: number;
}

export function limitOf(limits: readonly ParamLimit[] | undefined, name: string): NumberLimit {
  const found = limits?.find((limit) => limit.name === name);
  if (found === undefined) {
    return {};
  }
  return { min: found.min, max: found.max, step: found.step ?? undefined };
}

export function scaledLimit(limit: NumberLimit, factor: number): NumberLimit {
  return {
    min: limit.min === undefined ? undefined : limit.min * factor,
    max: limit.max === undefined ? undefined : limit.max * factor,
    step: limit.step === undefined ? undefined : limit.step * factor,
  };
}

export const AUDIO_DEFAULTS = {
  blankerThreshold: 5,
  clickThreshold: 6,
  squelchAutoMarginDb: 8,
  denoiseStrength: 0.5,
  filterLowHz: 300,
  filterHighHz: 3_000,
  notchFreqHz: 1_000,
  notchWidthHz: 100,
} as const;

export const AUDIO_LIMITS = {
  maxNotches: 4,
  blankerThreshold: { min: 1.5, max: 20 },
  clickThreshold: { min: 2, max: 20 },
  squelchAutoMarginDb: { min: 2, max: 40 },
  toneHz: { min: 30, max: 20_000 },
  notchWidthHz: { min: 10, max: 2_000 },
} as const;

export function mergeAudio(
  current: AudioProcessing,
  edit: Partial<AudioProcessing>,
): AudioProcessing {
  return { ...current, ...edit };
}

export function withNotchAdded(notches: NotchSettings[]): NotchSettings[] | null {
  if (notches.length >= AUDIO_LIMITS.maxNotches) {
    return null;
  }
  return [
    ...notches,
    { freq_hz: AUDIO_DEFAULTS.notchFreqHz, width_hz: AUDIO_DEFAULTS.notchWidthHz },
  ];
}

export function withNotchAt(
  notches: NotchSettings[],
  index: number,
  edit: Partial<NotchSettings>,
): NotchSettings[] {
  return notches.map((notch, at) => (at === index ? { ...notch, ...edit } : notch));
}

export function withNotchRemoved(notches: NotchSettings[], index: number): NotchSettings[] {
  return notches.filter((_, at) => at !== index);
}

export function audioChainActive(audio: AudioProcessing | undefined): boolean {
  if (audio === undefined) {
    return false;
  }
  return (
    (audio.click_removal?.enabled ?? false) ||
    (audio.filter?.enabled ?? false) ||
    (audio.notches?.length ?? 0) > 0 ||
    (audio.auto_notch ?? false) ||
    (audio.denoise?.enabled ?? false) ||
    (audio.agc ?? "off") !== "off"
  );
}

export function channelHasAudio(descriptor: ChannelDescriptor | undefined): boolean {
  return descriptor?.has_audio ?? true;
}

export function channelDecoderKind(descriptor: ChannelDescriptor | undefined): string | null {
  return descriptor?.decoder_kind ?? null;
}

export function channelHasVideo(descriptor: ChannelDescriptor | undefined): boolean {
  return descriptor?.has_video ?? false;
}

export interface RadioWindow {
  lowHz: number;
  highHz: number;
}

/// What a radio can hear right now, edge to edge, with room for a channel of this width.
export function radioWindowHz(
  centerHz: number | null | undefined,
  spanHz: number | null | undefined,
  descriptor: ChannelDescriptor | undefined,
): RadioWindow | null {
  const limitHz = offsetLimitHz(spanHz, descriptor);
  if (limitHz === null || centerHz == null || !Number.isFinite(centerHz)) {
    return null;
  }
  return { lowHz: centerHz - limitHz, highHz: centerHz + limitHz };
}

export function reachesHz(frequencyHz: number, window: RadioWindow | null): boolean {
  return window === null || (frequencyHz >= window.lowHz && frequencyHz <= window.highHz);
}

export function paramBandwidthHz(params: ChannelParams): number | null {
  return "bandwidth_hz" in params.settings && typeof params.settings.bandwidth_hz === "number"
    ? params.settings.bandwidth_hz
    : null;
}

export function channelWidthHz(
  params: ChannelParams | undefined,
  descriptor: ChannelDescriptor | undefined,
): number | null {
  const width =
    (params === undefined ? null : paramBandwidthHz(params)) ?? descriptor?.bandwidth_hz;
  return width !== undefined && Number.isFinite(width) && width > 0 ? width : null;
}

export function offsetLimitHz(
  spanHz: number | null | undefined,
  descriptor: ChannelDescriptor | undefined,
): number | null {
  if (spanHz == null || !Number.isFinite(spanHz) || spanHz <= 0) {
    return null;
  }
  return Math.max(0, (spanHz - (descriptor?.bandwidth_hz ?? 0)) / 2);
}

export function clampOffsetHz(hz: number, limitHz: number | null): number {
  return limitHz === null ? hz : Math.min(limitHz, Math.max(-limitHz, hz));
}

export function offsetForFrequencyHz(
  frequencyHz: number,
  centerHz: number,
  limitHz: number | null,
): number | null {
  if (!Number.isFinite(frequencyHz) || !Number.isFinite(centerHz)) {
    return null;
  }
  const offsetHz = Math.round(frequencyHz - centerHz);
  return limitHz !== null && Math.abs(offsetHz) > limitHz ? null : offsetHz;
}
