// Convenience aliases over the generated OpenAPI schema. These are the ONLY wire types —
// hand-writing a mirror of a Rust struct is forbidden (CLAUDE.md #1); regenerate instead.
import type { components } from "../generated/schema";

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
export type ServerEvent = components["schemas"]["ServerEvent"];
export type StreamKind = components["schemas"]["StreamKind"];
export type ClientCommand = components["schemas"]["ClientCommand"];
export type ApiError = components["schemas"]["ApiError"];
