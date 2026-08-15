import type { DeviceSet, TemplateInfo } from "../lib/types";
import { deviceId } from "./devices";

export function supports(template: TemplateInfo, set: DeviceSet | null): boolean {
  return set !== null && (template.supported_devices ?? []).includes(deviceId(set.device));
}
