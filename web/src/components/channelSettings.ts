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

/** The rate this channel needs, when the receiver is not running it — otherwise `null`.
 *
 * A mode that fills its whole channel rate has no guard band to resample through (PLAN §18), so
 * a radio retuned to another rate after the wire was drawn stops feeding it. `connectionRefusal`
 * catches that at drag time; this is the same rule for a pairing that has already been made. */
export function exactRateMismatch(
  descriptor: ChannelDescriptor | undefined,
  sampleRateHz: number | null | undefined,
): number | null {
  if (descriptor?.exact_rate_only !== true || sampleRateHz == null) {
    return null;
  }
  return sampleRateHz === descriptor.input_rate_hz ? null : descriptor.input_rate_hz;
}
