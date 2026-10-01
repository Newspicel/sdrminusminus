import { FaceFault } from "../../components/face/Fault";
import { Stat } from "../../components/face/Stats";
import { formatCount } from "../../components/format";
import type { ProcessorStatus } from "../../lib/types";
import { processorFaults } from "./processorFace";

export interface Chip {
  label: string;
  title: string;
  danger?: boolean;
}

export function ProcessorError({ status }: { status: ProcessorStatus | null }) {
  const error = status?.error ?? null;
  return error === null ? null : <FaceFault message={error} />;
}

export function ProcessorStats({
  status,
  chips = [],
}: {
  status: ProcessorStatus | null;
  chips?: readonly Chip[];
}) {
  return (
    <>
      {chips.map((chip) => (
        <Stat
          key={chip.label}
          label={chip.label}
          title={chip.title}
          tone={chip.danger === true ? "danger" : "warn"}
        />
      ))}
      {processorFaults(status).map((row) => (
        <Stat key={row.label} label={row.label} title={row.title} tone="danger">
          {formatCount(row.count)}
        </Stat>
      ))}
    </>
  );
}
