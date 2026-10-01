import type { ReactNode } from "react";
import { FaceStats } from "../../components/face/Stats";
import type { ProcessorStatus } from "../../lib/types";
import { FaceFooter } from "./NodeShell";
import { type Chip, ProcessorStats } from "./ProcessorHealth";
import { processorFaults } from "./processorFace";

export function ProcessorFooter({
  status,
  chips = [],
  actions,
}: {
  status: ProcessorStatus | null;
  chips?: readonly Chip[];
  actions?: ReactNode;
}) {
  if (chips.length === 0 && processorFaults(status).length === 0 && actions === undefined) {
    return null;
  }
  return (
    <FaceFooter>
      <FaceStats>
        <ProcessorStats status={status} chips={chips} />
      </FaceStats>
      {actions}
    </FaceFooter>
  );
}
