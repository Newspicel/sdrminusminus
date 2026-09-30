import type { Colormap } from "../gl/colormap";
import { colormapGradient, type PlotRect, type Tick } from "./plotFrame";

const TICK_PX = 3;
const LABEL_GAP_PX = 2;

export function PlotAxis({
  side,
  ticks,
  plot,
}: {
  side: "left" | "right" | "bottom";
  ticks: readonly Tick[];
  plot: PlotRect;
}) {
  if (side === "bottom") {
    const y = plot.y + plot.h;
    return (
      <g className="fill-ink-faint stroke-line-strong">
        {ticks
          .filter((tick) => tick.px >= 0 && tick.px <= plot.w)
          .map((tick) => (
            <g key={tick.label}>
              <line x1={plot.x + tick.px} x2={plot.x + tick.px} y1={y} y2={y + TICK_PX} />
              <text
                x={plot.x + tick.px}
                y={y + TICK_PX + LABEL_GAP_PX}
                textAnchor="middle"
                dominantBaseline="hanging"
                className="stroke-none font-mono text-[9px]"
              >
                {tick.label}
              </text>
            </g>
          ))}
      </g>
    );
  }
  const left = side === "left";
  const x = left ? plot.x : plot.x + plot.w;
  const out = left ? -TICK_PX : TICK_PX;
  return (
    <g className="fill-ink-faint stroke-line-strong">
      {ticks
        .filter((tick) => tick.px >= 0 && tick.px <= plot.h)
        .map((tick) => (
          <g key={tick.label}>
            <line x1={x} x2={x + out} y1={plot.y + tick.px} y2={plot.y + tick.px} />
            <text
              x={x + out + (left ? -LABEL_GAP_PX : LABEL_GAP_PX)}
              y={plot.y + tick.px}
              textAnchor={left ? "end" : "start"}
              dominantBaseline="middle"
              className="stroke-none font-mono text-[9px]"
            >
              {tick.label}
            </text>
          </g>
        ))}
    </g>
  );
}

export function ColourScale({
  colormap,
  min,
  max,
  unit,
}: {
  colormap: Colormap;
  min: number;
  max: number;
  unit: string;
}) {
  return (
    <span
      className="flex items-center gap-1 font-mono text-[9px] tabular-nums text-ink-faint"
      title="Colour scale"
    >
      <span>{min.toFixed(0)}</span>
      <span
        className="h-1.5 w-16 rounded-[1px]"
        style={{ background: colormapGradient(colormap, "to right") }}
      />
      <span>{max.toFixed(0)}</span>
      <span>{unit}</span>
    </span>
  );
}
