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

// An empty per-type settings object deserializes with every field at its server-side default
// (the wire enum is built that way), so "add channel" needs only the tag. Keyed by the union's
// tag, so a new variant in the generated schema fails to compile until it is listed here.
const EMPTY_PARAMS: { [K in ChannelTypeId]: () => Extract<ChannelParams, { type: K }> } = {
  nfm: () => ({ type: "nfm", settings: {} }),
  am: () => ({ type: "am", settings: {} }),
  ssb: () => ({ type: "ssb", settings: {} }),
  wfm: () => ({ type: "wfm", settings: {} }),
  pocsag: () => ({ type: "pocsag", settings: {} }),
  adsb: () => ({ type: "adsb", settings: {} }),
  ais: () => ({ type: "ais", settings: {} }),
  aprs: () => ({ type: "aprs", settings: {} }),
  rtty: () => ({ type: "rtty", settings: {} }),
  morse: () => ({ type: "morse", settings: {} }),
};

export function isChannelTypeId(typeId: string): typeId is ChannelTypeId {
  return Object.hasOwn(EMPTY_PARAMS, typeId);
}

export function defaultChannelSettings(typeId: string): ChannelSettings | null {
  if (!isChannelTypeId(typeId)) {
    return null;
  }
  return { offset_hz: 0, params: EMPTY_PARAMS[typeId]() };
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
