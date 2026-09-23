import { formatBaud, formatHz, formatSampleRate } from "../../components/format";
import type { IqFrame, SymbolFrame } from "../../lib/frame";

export interface Measurement {
  label: string;
  value: string;
  hint?: string;
}

export function measurements(
  view: string,
  frame: IqFrame | null,
  block: SymbolFrame | null,
  period: number,
): Measurement[] {
  return [...signalRows(view, frame, block, period), ...symbolRows(view, block)];
}

function signalRows(
  view: string,
  frame: IqFrame | null,
  block: SymbolFrame | null,
  period: number,
): Measurement[] {
  if (frame === null) {
    return [];
  }
  const rows: Measurement[] = [
    { label: "Centre", value: formatHz(frame.centerHz) },
    { label: "Rate", value: formatSampleRate(frame.sampleRate) },
  ];
  const folded = view === "eye" || (block === null && view !== "spectrum");
  if (folded && period > 0) {
    rows.push({
      label: "Sam/sym",
      value: period.toFixed(2),
      hint: "Samples per symbol at the chosen symbol rate",
    });
  }
  return rows;
}

function symbolRows(view: string, block: SymbolFrame | null): Measurement[] {
  if (block === null || view === "spectrum" || view === "eye") {
    return [];
  }
  const offset = Math.round(block.freqErrorHz);
  return [
    { label: "Symbols", value: formatBaud(block.symbolRate) },
    {
      label: "EVM",
      value: `${(block.evm * 100).toFixed(1)} %`,
      hint: "Error vector magnitude, lower is better",
    },
    {
      label: "MER",
      value: block.merDb >= 99 ? "clean" : `${block.merDb.toFixed(1)} dB`,
      hint: "Modulation error ratio, higher is better",
    },
    {
      label: "Margin",
      value: `×${block.margin.toFixed(2)}`,
      hint: "Distance to the nearest decision threshold",
    },
    {
      label: "Offset",
      value: `${offset > 0 ? "+" : ""}${offset} Hz`,
      hint: "Carrier frequency error",
    },
  ];
}
