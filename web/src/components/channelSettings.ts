import type {
  AudioProcessing,
  ChannelDescriptor,
  ChannelParams,
  ChannelSettings,
  NotchSettings,
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
    offset_hz: edit.offset_hz ?? current.offset_hz ?? 0,
    squelch_db: edit.squelch_db !== undefined ? edit.squelch_db : (current.squelch_db ?? null),
    squelch_auto_db:
      edit.squelch_auto_db !== undefined ? edit.squelch_auto_db : (current.squelch_auto_db ?? null),
    params: edit.params ?? current.params,
    audio: edit.audio ?? current.audio ?? {},
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
  current: ChannelSettings,
  edit: Partial<AudioProcessing>,
): AudioProcessing {
  return { ...current.audio, ...edit };
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
    (audio.blanker?.enabled ?? false) ||
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

export function rateMismatch(
  descriptor: ChannelDescriptor | undefined,
  sampleRateHz: number | null | undefined,
): { min: number; max: number } | null {
  if (descriptor === undefined || sampleRateHz == null) {
    return null;
  }
  const wanted = rateRange(descriptor);
  if (wanted === null || (sampleRateHz >= wanted.min && sampleRateHz <= wanted.max)) {
    return null;
  }
  return wanted;
}

export function rateRange(descriptor: ChannelDescriptor): { min: number; max: number } | null {
  if (descriptor.native_rate_max_hz != null) {
    return { min: descriptor.input_rate_hz, max: descriptor.native_rate_max_hz };
  }
  return descriptor.exact_rate_only
    ? { min: descriptor.input_rate_hz, max: descriptor.input_rate_hz }
    : null;
}
