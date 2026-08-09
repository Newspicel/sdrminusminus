// Convenience aliases over the generated OpenAPI schema. These are the ONLY wire types —
// hand-writing a mirror of a Rust struct is forbidden (CLAUDE.md #1); regenerate instead.
import type { components, operations } from "../generated/schema";

export type StateSnapshot = components["schemas"]["StateSnapshot"];
export type StateScope = components["schemas"]["StateScope"];
export type DeviceSet = components["schemas"]["DeviceSet"];
export type DeviceInfo = components["schemas"]["DeviceInfo"];
export type DeviceSettings = components["schemas"]["DeviceSettings"];
export type Capabilities = components["schemas"]["Capabilities"];
export type GainStage = components["schemas"]["GainStage"];
export type ExtraSetting = components["schemas"]["ExtraSetting"];
export type DevicesResponse = components["schemas"]["DevicesResponse"];
export type ChannelInfo = components["schemas"]["ChannelInfo"];
export type ChannelSettings = components["schemas"]["ChannelSettings"];
export type ChannelParams = components["schemas"]["ChannelParams"];
export type ChannelDescriptor = components["schemas"]["ChannelDescriptor"];
export type ChannelTypesResponse = components["schemas"]["ChannelTypesResponse"];
export type PresetInfo = components["schemas"]["PresetInfo"];
export type Bookmark = components["schemas"]["Bookmark"];
export type CreateBookmarkRequest = components["schemas"]["CreateBookmarkRequest"];
export type RecordAction = components["schemas"]["RecordAction"];
export type RecordingStatus = components["schemas"]["RecordingStatus"];
export type RecordingInfo = components["schemas"]["RecordingInfo"];
export type RecordingsResponse = components["schemas"]["RecordingsResponse"];
export type DecodedRecord = components["schemas"]["DecodedRecord"];
export type DecoderEvent = components["schemas"]["DecoderEvent"];
export type DecoderLogEntry = components["schemas"]["DecoderLogEntry"];
export type DecoderLogResponse = components["schemas"]["DecoderLogResponse"];
export type DeletedCount = components["schemas"]["DeletedCount"];
export type ExportFormat = components["schemas"]["ExportFormat"];
export type AdsbMessage = components["schemas"]["AdsbMessage"];
export type AisMessage = components["schemas"]["AisMessage"];
export type AprsPacket = components["schemas"]["AprsPacket"];
export type PocsagMessage = components["schemas"]["PocsagMessage"];
export type RdsUpdate = components["schemas"]["RdsUpdate"];
export type RttyText = components["schemas"]["RttyText"];
export type MorseText = components["schemas"]["MorseText"];
export type ServerEvent = components["schemas"]["ServerEvent"];
export type StreamKind = components["schemas"]["StreamKind"];
export type ClientCommand = components["schemas"]["ClientCommand"];
export type ApiError = components["schemas"]["ApiError"];

// `DecoderLogQuery` is flattened into the operation's query parameters by utoipa, so it has no
// `components.schemas` entry — deriving it from `operations` keeps it generated either way.
export type DecoderLogFilter = NonNullable<operations["list_decoder_log"]["parameters"]["query"]>;

// Discriminant and per-variant projections of the generated `DecoderEvent` union, so a panel or
// store can be typed on one decoder without re-declaring its payload.
export type DecoderKind = DecoderEvent["kind"];
export type DecoderEventOf<K extends DecoderKind> = Extract<DecoderEvent, { kind: K }>;
export type DecodedRecordOf<K extends DecoderKind> = Omit<DecodedRecord, "event"> & {
  event: DecoderEventOf<K>;
};
