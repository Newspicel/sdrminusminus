import type { ChannelSettings } from "../lib/types";

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
// (the wire enum is built that way), so "add channel" needs only the tag.
export function defaultChannelSettings(typeId: string): ChannelSettings | null {
  switch (typeId) {
    case "nfm":
      return { offset_hz: 0, params: { type: "nfm", settings: {} } };
    case "am":
      return { offset_hz: 0, params: { type: "am", settings: {} } };
    case "ssb":
      return { offset_hz: 0, params: { type: "ssb", settings: {} } };
    case "wfm":
      return { offset_hz: 0, params: { type: "wfm", settings: {} } };
    default:
      return null;
  }
}
