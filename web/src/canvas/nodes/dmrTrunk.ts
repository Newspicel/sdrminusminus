// The protocol list a DMR trunk face offers and the guidance it prints under it. Kept out of
// `DmrTrunkFace.tsx` so that file exports only components — a mixed module costs Fast Refresh the
// component state it would otherwise preserve.
import type { DmrTrunkProtocol, DvTrunkProtocol } from "../../lib/types";

export const DMR_TRUNK_PROTOCOLS: readonly { value: DmrTrunkProtocol; label: string }[] = [
  { value: "auto", label: "Auto-detect" },
  { value: "capacity_plus", label: "Capacity Plus" },
  { value: "hytera_xpt", label: "Hytera XPT" },
  { value: "tier_three", label: "Tier III / Capacity Max" },
];

export function dmrTrunkGuidance(
  protocol: DmrTrunkProtocol,
  detected: DvTrunkProtocol | null = null,
): string {
  if (protocol === "auto" && detected !== null) {
    switch (detected) {
      case "capacity_plus":
        return "Detected Capacity Plus signalling; both timeslots of every wired carrier are being followed.";
      case "hytera_xpt":
        return "Detected Hytera XPT signalling; both timeslots of every wired carrier are being followed.";
      case "tier_three":
        return "Detected Tier III signalling; voice grants create traffic receivers automatically.";
    }
  }
  switch (protocol) {
    case "capacity_plus":
      return "Add one DMR decoder for every known repeater output frequency. Both timeslots are isolated automatically.";
    case "hytera_xpt":
      return "Add one DMR decoder for every Hytera XPT repeater output frequency. Both timeslots are isolated automatically.";
    case "tier_three":
      return "Add the DMR control-channel decoder. Standard channel definitions and voice grants create traffic receivers automatically.";
    case "auto":
      return "The system detects Capacity Plus, Hytera XPT, or Tier III signalling from the connected DMR carriers.";
  }
}
