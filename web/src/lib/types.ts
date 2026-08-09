// Convenience aliases over the generated OpenAPI schema. These are the ONLY wire types —
// hand-writing a mirror of a Rust struct is forbidden (CLAUDE.md #1); regenerate instead.
import type { components } from "../generated/schema";

export type StateSnapshot = components["schemas"]["StateSnapshot"];
export type DeviceSet = components["schemas"]["DeviceSet"];
export type DeviceInfo = components["schemas"]["DeviceInfo"];
export type DeviceSettings = components["schemas"]["DeviceSettings"];
export type Capabilities = components["schemas"]["Capabilities"];
export type DevicesResponse = components["schemas"]["DevicesResponse"];
export type ServerEvent = components["schemas"]["ServerEvent"];
export type ClientCommand = components["schemas"]["ClientCommand"];
export type ApiError = components["schemas"]["ApiError"];
