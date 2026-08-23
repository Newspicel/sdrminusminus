import { LABEL, TABLE_CELL } from "../../components/controls";
import type { ConversionReport } from "../../lib/types";
import { countsLine, groupIssues, issueLine, reportSummary } from "./cps";

const TONE = {
  dropped: "text-danger",
  adjusted: "text-warn",
  note: "text-ink-dim",
} as const;

export function ReportView({ report }: { report: ConversionReport | null }) {
  if (report === null) {
    return null;
  }
  const groups = groupIssues(report);
  return (
    <section className="flex flex-col gap-2 rounded-[3px] border border-line bg-panel-2 p-3">
      <header className="flex flex-wrap items-baseline justify-between gap-2">
        <h4 className={LABEL}>Fit for {report.target_model}</h4>
        <span className="font-mono text-xs text-ink">{reportSummary(report)}</span>
      </header>
      <p className="font-mono text-[11px] text-ink-dim">{countsLine(report)}</p>
      {groups.map((group) => (
        <div key={group.severity} className="flex flex-col gap-0.5">
          <h5 className={`${LABEL} ${TONE[group.severity]}`}>
            {group.label} ({group.issues.length})
          </h5>
          <ul className="flex flex-col">
            {group.issues.slice(0, 40).map((issue) => (
              <li key={issueLine(issue)} className={`${TABLE_CELL} text-ink-dim`}>
                {issueLine(issue)}
              </li>
            ))}
            {group.issues.length > 40 && (
              <li className={`${TABLE_CELL} text-ink-faint`}>
                and {group.issues.length - 40} more
              </li>
            )}
          </ul>
        </div>
      ))}
    </section>
  );
}
