const MAX_TICKS = 1_000;

export function niceStep(span: number, target: number): number {
  if (!(span > 0) || !(target > 0) || !Number.isFinite(span)) {
    return 0;
  }
  const raw = span / target;
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / magnitude;
  const factor = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
  return factor * magnitude;
}

export function stepDecimals(step: number): number {
  if (!(step > 0) || !Number.isFinite(step)) {
    return 0;
  }
  return Math.max(0, -Math.floor(Math.log10(step) + 1e-9));
}

export function niceTicks(min: number, max: number, target: number): number[] {
  const low = Math.min(min, max);
  const high = Math.max(min, max);
  const step = niceStep(high - low, target);
  if (step === 0) {
    return Number.isFinite(low) ? [low] : [];
  }
  const decimals = stepDecimals(step);
  const first = Math.ceil(low / step - 1e-9);
  const ticks: number[] = [];
  for (let index = first; ticks.length < MAX_TICKS; index++) {
    const value = Number((index * step).toFixed(decimals));
    if (value > high + step * 1e-9) {
      break;
    }
    ticks.push(Object.is(value, -0) ? 0 : value);
  }
  return ticks;
}

export function tickLabel(value: number, step: number): string {
  return value.toFixed(stepDecimals(step));
}
