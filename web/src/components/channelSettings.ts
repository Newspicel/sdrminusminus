import type {
  AudioProcessing,
  ChannelDescriptor,
  ChannelParams,
  ChannelSettings,
  NotchSettings,
} from "../lib/types";

/** Tag of the generated `ChannelParams` union — the stable `ChannelDescriptor.type_id`. */
export type ChannelTypeId = ChannelParams["type"];

/** Per-variant settings shape, projected from the generated union rather than re-declared. */
export type ChannelParamsOf<K extends ChannelTypeId> = Extract<
  ChannelParams,
  { type: K }
>["settings"];

// `PATCH /channels/{ch}` replaces the whole settings object (unlike the device's field-merge
// PATCH), so every edit must be widened to a full `ChannelSettings` over the current value.
// `squelch_db: undefined` means "unchanged"; `null` means "squelch open".
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

/** Server defaults for every stage, so a control reads the same number the engine would use for
 * a field a peer left out. Kept beside `mergeChannelSettings` because both exist for the same
 * reason: the wire makes every field optional and the panel has to render one anyway. */
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

/** Bounds the server enforces (`sdrmm_wire::audio`); a control outside them is a refused PATCH. */
export const AUDIO_LIMITS = {
  maxNotches: 4,
  blankerThreshold: { min: 1.5, max: 20 },
  clickThreshold: { min: 2, max: 20 },
  squelchAutoMarginDb: { min: 2, max: 40 },
  toneHz: { min: 30, max: 20_000 },
  notchWidthHz: { min: 10, max: 2_000 },
} as const;

/** Widen a partial audio-chain edit over the channel's current chain. The chain is one field of
 * `ChannelSettings`, so a stage edit has to carry the other stages with it. */
export function mergeAudio(
  current: ChannelSettings,
  edit: Partial<AudioProcessing>,
): AudioProcessing {
  return { ...current.audio, ...edit };
}

/** A notch appended at the default frequency, or `null` once the channel is full. */
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

/** Whether any stage of the chain is doing something — what the panel's summary reports, and
 * the mirror of `AudioProcessing::is_active` on the server. */
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

// A decoder channel is not automatically silent — WFM decodes RDS while still producing audio —
// so only the descriptor's flag decides. Absent (older server, or the type list not loaded yet)
// keeps the pre-M4 behaviour of offering audio.
export function channelHasAudio(descriptor: ChannelDescriptor | undefined): boolean {
  return descriptor?.has_audio ?? true;
}

/** The `DecoderEvent.kind` this channel emits, or null when it is a plain demod. */
export function channelDecoderKind(descriptor: ChannelDescriptor | undefined): string | null {
  return descriptor?.decoder_kind ?? null;
}

/** Whether this channel scans out a picture, so its face mounts a video panel and subscribes.
 * Absent (older server, or the type list not loaded yet) means no video, which is every mode
 * that predates the transport. */
export function channelHasVideo(descriptor: ChannelDescriptor | undefined): boolean {
  return descriptor?.has_video ?? false;
}

/** How far the channel can be offset before its passband leaves the receiver's span: half the
 * span, less the half-bandwidth the channel itself occupies. `null` when the rate is unknown —
 * the offset field is then left unbounded rather than clamped to a guess. */
export function offsetLimitHz(
  spanHz: number | null | undefined,
  descriptor: ChannelDescriptor | undefined,
): number | null {
  if (spanHz == null || !Number.isFinite(spanHz) || spanHz <= 0) {
    return null;
  }
  return Math.max(0, (spanHz - (descriptor?.bandwidth_hz ?? 0)) / 2);
}

/** Holds an offset inside `offsetLimitHz`. An offset past the edge of the span tunes the channel
 * to nothing, so the steppers stop there rather than walking off the band. */
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

/** The rates this type admits, or `null` when it takes whatever the DDC can resample to. */
export function rateRange(descriptor: ChannelDescriptor): { min: number; max: number } | null {
  if (descriptor.native_rate_max_hz != null) {
    return { min: descriptor.input_rate_hz, max: descriptor.native_rate_max_hz };
  }
  return descriptor.exact_rate_only
    ? { min: descriptor.input_rate_hz, max: descriptor.input_rate_hz }
    : null;
}
