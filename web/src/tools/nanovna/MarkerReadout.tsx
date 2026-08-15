import { LABEL } from "../../components/controls";
import { formatHz } from "../../components/format";
import type { Band, PointReadout, SweepAnalysis } from "./analysis";
import { formatDb, formatImpedance, formatNumber, formatSi, formatVswr } from "./nanovna";

export function MarkerReadout({ row }: { row: PointReadout }) {
  const z = row.impedance;
  const y = row.admittance;
  return (
    <div className="grid gap-x-6 gap-y-3 sm:grid-cols-2 xl:grid-cols-3">
      <Group title={`Reflection · ${formatHz(row.frequencyHz)}`}>
        <Entry label="VSWR" value={formatVswr(row.vswr)} accent />
        <Entry label="Return loss" value={formatDb(row.returnLossDb)} />
        <Entry label="|S11|" value={formatDb(row.s11Db)} />
        <Entry label="|S11| linear" value={formatNumber(row.s11Linear, 5)} />
        <Entry label="S11 phase" value={`${formatNumber(row.s11PhaseDeg, 2)}°`} />
        <Entry label="Mismatch loss" value={formatDb(row.mismatchLossDb)} />
        <Entry label="S11 real" value={formatNumber(row.s11.re, 6)} />
        <Entry label="S11 imag" value={formatNumber(row.s11.im, 6)} />
      </Group>
      <Group title="Impedance">
        <Entry label="Z" value={formatImpedance(z)} accent />
        <Entry label="Resistance" value={z === null ? "—" : `${formatNumber(z.re, 2)} Ω`} />
        <Entry label="Reactance" value={z === null ? "—" : `${formatNumber(z.im, 2)} Ω`} />
        <Entry label="|Z|" value={`${formatNumber(row.impedanceMagnitude, 2)} Ω`} />
        <Entry label="Q" value={formatNumber(row.q, 2)} />
        <Entry
          label={row.component?.kind === "inductance" ? "Series L" : "Series C"}
          value={
            row.component === null
              ? "—"
              : formatSi(row.component.value, row.component.kind === "inductance" ? "H" : "F")
          }
        />
        <Entry label="Conductance" value={y === null ? "—" : formatSi(y.re, "S")} />
        <Entry label="Susceptance" value={y === null ? "—" : formatSi(y.im, "S")} />
      </Group>
      <Group title="Transmission">
        <Entry label="S21 gain" value={formatDb(row.s21Db)} accent />
        <Entry label="Insertion loss" value={formatDb(row.insertionLossDb)} />
        <Entry label="|S21| linear" value={formatNumber(row.s21Linear, 6)} />
        <Entry label="S21 phase" value={`${formatNumber(row.s21PhaseDeg, 2)}°`} />
        <Entry label="Group delay" value={formatSi(row.groupDelayS, "s", 2)} />
        <Entry label="S21 real" value={formatNumber(row.s21.re, 6)} />
        <Entry label="S21 imag" value={formatNumber(row.s21.im, 6)} />
        <Entry label="Point" value={`#${row.index + 1}`} />
      </Group>
    </div>
  );
}

export function SweepSummary({ analysis }: { analysis: SweepAnalysis }) {
  const resonance = analysis.resonance;
  return (
    <div className="grid gap-x-6 gap-y-3 sm:grid-cols-2 xl:grid-cols-3">
      <Group title="Best match">
        {resonance === null ? (
          <Entry label="Resonance" value="—" />
        ) : (
          <>
            <Entry label="Frequency" value={formatHz(resonance.frequencyHz)} accent />
            <Entry label="VSWR" value={formatVswr(resonance.vswr)} />
            <Entry label="Return loss" value={formatDb(resonance.returnLossDb)} />
            <Entry label="Z" value={formatImpedance(resonance.impedance)} />
          </>
        )}
      </Group>
      <Group title="Usable bandwidth">
        {analysis.vswrBands.map(({ limit, band }) => (
          <Entry
            key={limit}
            label={`VSWR ≤ ${limit}`}
            value={band === null ? "not reached" : describeBand(band)}
          />
        ))}
      </Group>
      <Group title="Transmission">
        {analysis.peak === null ? (
          <Entry label="S21" value="nothing through CH1" />
        ) : (
          <>
            <Entry label="Peak" value={formatDb(analysis.peak.s21Db)} accent />
            <Entry label="At" value={formatHz(analysis.peak.frequencyHz)} />
            <Entry
              label="−3 dB band"
              value={
                analysis.transmissionBand === null
                  ? "not reached"
                  : describeBand(analysis.transmissionBand)
              }
            />
            <Entry
              label="Loaded Q"
              value={
                analysis.transmissionBand === null
                  ? "—"
                  : formatNumber(analysis.transmissionBand.q, 1)
              }
            />
          </>
        )}
      </Group>
    </div>
  );
}

/** A band that ran into the end of the sweep is reported as at-least, never as a measurement:
 * the real edge is outside what was swept. */
function describeBand(band: Band): string {
  const span = `${formatHz(band.startHz)} – ${formatHz(band.stopHz)}`;
  const width = formatHz(band.spanHz);
  return band.truncated ? `${span} (≥ ${width}, clipped)` : `${span} (${width})`;
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
