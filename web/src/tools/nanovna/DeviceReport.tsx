import { LABEL } from "../../components/controls";
import { formatHz } from "../../components/format";
import type { NanoVnaDeviceReport } from "../../lib/types";
import { formatSi } from "./nanovna";

export function DeviceReport({ report }: { report: NanoVnaDeviceReport }) {
  const sweep = report.sweep;
  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-x-6 gap-y-3 sm:grid-cols-2 xl:grid-cols-3">
        <Group title="Instrument">
          <Entry label="Board" value={report.board ?? "unnamed"} accent />
          <Entry label="Firmware" value={report.firmware} />
          <Entry label="Port" value={report.port} />
          <Entry
            label="Battery"
            value={report.battery_mv == null ? "—" : `${(report.battery_mv / 1000).toFixed(3)} V`}
          />
        </Group>
        <Group title="Measurement">
          <Entry
            label="IF bandwidth"
            value={report.bandwidth_hz == null ? "—" : `${report.bandwidth_hz} Hz`}
          />
          <Entry label="Drive level" value={describePower(report.power)} />
          <Entry
            label="Electrical delay"
            value={
              report.electrical_delay_s == null ? "—" : formatSi(report.electrical_delay_s, "s", 3)
            }
          />
          <Entry
            label="S21 offset"
            value={report.s21_offset_db == null ? "—" : `${report.s21_offset_db.toFixed(3)} dB`}
          />
        </Group>
        <Group title="Reference and range">
          <Entry label="TCXO" value={report.tcxo_hz == null ? "—" : formatHz(report.tcxo_hz)} />
          <Entry
            label="Harmonic above"
            value={
              report.harmonic_threshold_hz == null ? "—" : formatHz(report.harmonic_threshold_hz)
            }
          />
          <Entry
            label="Device sweep"
            value={sweep == null ? "—" : `${formatHz(sweep.start_hz)} – ${formatHz(sweep.stop_hz)}`}
          />
          <Entry label="Device points" value={sweep == null ? "—" : String(sweep.points)} />
        </Group>
      </div>

      <section className="flex flex-col gap-1">
        <h4 className={LABEL}>Calibration</h4>
        <p className="font-mono text-xs text-ink">
          {report.calibration.raw === "" ? "no calibration in memory" : report.calibration.raw}
        </p>
      </section>

      {report.info.length > 0 && (
        <section className="flex flex-col gap-1">
          <h4 className={LABEL}>Reported by the device</h4>
          <pre className="overflow-x-auto rounded-[3px] border border-line bg-panel-2 p-2 font-mono text-[11px] leading-relaxed text-ink-dim">
            {report.info.join("\n")}
          </pre>
        </section>
      )}

      {report.commands.length > 0 && (
        <section className="flex flex-col gap-1">
          <h4 className={LABEL}>Shell commands ({report.commands.length})</h4>
          <p className="font-mono text-[11px] leading-relaxed break-words text-ink-dim">
            {report.commands.join(" ")}
          </p>
        </section>
      )}
    </div>
  );
}

function describePower(power: number | null | undefined): string {
  if (power == null) {
    return "—";
  }
  return power === 255 ? "auto" : String(power);
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex min-w-0 flex-col gap-1">
      <h4 className={LABEL}>{title}</h4>
      <dl className="flex flex-col gap-0.5">{children}</dl>
    </section>
  );
}

function Entry({
  label,
  value,
  accent = false,
}: {
  label: string;
  value: string;
  accent?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-line/40 py-0.5">
      <dt className="shrink-0 text-xs text-ink-dim">{label}</dt>
      <dd
        className={`truncate font-mono text-xs tabular-nums ${accent ? "text-accent" : "text-ink"}`}
      >
        {value}
      </dd>
    </div>
  );
}
