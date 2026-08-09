// Large, digit-grouped frequency readout in tabular mono numerals (PLAN §10 design language).

export function FrequencyReadout({ hz }: { hz: number }) {
  return (
    <div className="font-mono tabular-nums leading-none">
      <span className="text-3xl tracking-tight text-ink">{formatMhz(hz)}</span>
      <span className="ml-2 text-sm text-ink-dim">MHz</span>
    </div>
  );
}

function formatMhz(hz: number): string {
  const parts = (hz / 1e6).toFixed(6).split(".");
  const whole = parts[0] ?? "0";
  const frac = parts[1] ?? "000000";
  return `${whole}.${frac.slice(0, 3)} ${frac.slice(3)}`.trimEnd();
}
