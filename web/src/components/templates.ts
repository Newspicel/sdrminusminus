import type { DeviceSet, TemplateInfo } from "../lib/types";
import { deviceId } from "./devices";

export function supports(template: TemplateInfo, set: DeviceSet | null): boolean {
  return set !== null && (template.supported_devices ?? []).includes(deviceId(set.device));
}

export function templatesHint(
  templates: readonly TemplateInfo[],
  set: DeviceSet | null,
): string | null {
  if (set === null) return "Select a device first.";
  if (templates.length > 0 && !templates.some((t) => supports(t, set))) {
    return `${set.device.label} cannot run these templates.`;
  }
  return null;
}
