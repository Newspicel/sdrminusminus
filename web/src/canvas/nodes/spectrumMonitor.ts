import type { DecodedRecordOf } from "../../lib/types";

type Transmission = DecodedRecordOf<"transmission">["event"]["data"];

export function monitorTransmissions(
  records: readonly DecodedRecordOf<"transmission">[],
  node: string,
): Transmission[] {
  const latest = new Map<number, Transmission>();
  for (const record of records) {
    if (record.origin?.node !== node || latest.has(record.event.data.id)) continue;
    latest.set(record.event.data.id, record.event.data);
  }
  return [...latest.values()];
}
