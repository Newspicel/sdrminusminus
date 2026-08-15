// Whether a radio can run a template. Kept out of `TemplatesPanel.tsx` so that file exports only
// components — a mixed module costs Fast Refresh the component state it would otherwise preserve.
import type { DeviceSet, TemplateInfo } from "../lib/types";
import { deviceId } from "./devices";

/** Whether this radio is one the server said can run the template.
 *
 * The rule itself is `TemplateInfo::unmet_by` in `crates/wire`, evaluated server-side against
 * every probed radio's profile — frequency span, sample rate and whether it has the direction
 * the template needs. This is the lookup, not a second copy of the rule: the engine's rejection
 * on apply is still the authority, but a card that says why beats a failed apply. */
export function supports(template: TemplateInfo, set: DeviceSet | null): boolean {
  return set !== null && (template.supported_devices ?? []).includes(deviceId(set.device));
}
