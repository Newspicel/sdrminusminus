import type { ChannelDescriptor, ChannelParams, ChannelSettings } from "../lib/types";

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
    params: edit.params ?? current.params,
  };
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

/** The rates a channel will run at, when the receiver is not running one of them — otherwise
 * `null`. Both ends inclusive, and equal when only one rate will do.
 *
 * Two rules, both the server's (): a mode that fills its whole channel rate has no guard
 * band to resample through and takes exactly one rate, and a mode that reads the radio's own
 * samples takes a range of them. Either way a radio retuned after the wire was drawn stops
 * feeding it, which is what this catches for a pairing already made. */
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
