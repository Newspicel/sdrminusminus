import type { DeviceSettings } from "../lib/types";

export function followOffset(current: DeviceSettings, delta: DeviceSettings): DeviceSettings {
  if (delta.offset_hz == null || delta.center_hz != null || current.center_hz == null) {
    return delta;
  }
  const moved = delta.offset_hz - (current.offset_hz ?? 0);
  return { ...delta, center_hz: current.center_hz + moved };
}
