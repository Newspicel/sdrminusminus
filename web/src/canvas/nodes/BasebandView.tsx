// The scope's other instrument: one channel's passband, as the demodulator sees it.
//
// Three views over the same burst — the spectrum of the channel (real resolution, not a re-framing
// of the device's bins), the constellation, and the eye. Which one is useful depends entirely on
// what is being received, so they are switched rather than stacked.

import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "../../components/BaseControls";
import {
  addConstellation,
  addEye,
  type BasebandGrid,
  clearBasebandGrid,
  createBasebandGrid,
  decayBasebandGrid,
  EYE_COMPONENTS,
  type EyeComponent,
  eyeScale,
  peakMagnitude,
  samplesPerSymbol,
} from "../../components/baseband";
import { plotButton, segment } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Popover } from "../../components/Popover";
import { colormapLut } from "../../components/persistence";
import { FULL_VIEW } from "../../components/spectrumView";
import type { Colormap } from "../../gl/colormap";
import { SpectrumAnalyzer } from "../../lib/dsp/fft";
import type { IqFrame } from "../../lib/frame";
import { iqHub } from "../../lib/iq";
import { token } from "../../lib/tokens";
import type { ChannelInfo } from "../../lib/types";
import { drawPlot, GridBitmap } from "./scopePlot";

export const BASEBAND_VIEWS = ["spectrum", "constellation", "eye"] as const;
export type BasebandView = (typeof BASEBAND_VIEWS)[number];

/** Transform size for the channel spectrum. A burst is 2048 samples, so this uses all of it. */
const FFT_SIZE = 2048;
/** The scatter grids. Square, because a constellation's axes are the same quantity. */
const GRID = 320;
/** dB below the burst's own peak that the channel spectrum shows. Fixed rather than adaptive: a
 * baseband plot is read for its shape against the noise floor, and a window that breathed with
 * every burst would make that shape impossible to compare. */
const SPECTRUM_RANGE_DB = 90;
/** What a symbol rate is clamped to. The upper bound is Nyquist, enforced against the channel's
 * own rate; the lower keeps the eye from folding on a period longer than the burst. */
const MIN_SYMBOL_RATE = 1;

export function BasebandView({
  deviceSet,
  channel,
  colormap,
  label,
}: {
  deviceSet: number;
  channel: ChannelInfo;
  colormap: Colormap;
  label: ReactNode;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [frame, setFrame] = useState<IqFrame | null>(() => iqHub.latest(deviceSet, channel.id));
  const frameRef = useRef<IqFrame | null>(frame);
  const [view, setView] = useState<BasebandView>("spectrum");
  const [eyeComponent, setEyeComponent] = useState<EyeComponent>("frequency");
  const [symbolRate, setSymbolRate] = useState(4800);
  /** Whether the constellation keeps only the decision points or the whole trajectory. */
  const [decimate, setDecimate] = useState(false);

  const gridRef = useRef<BasebandGrid | null>(null);
  const bitmapRef = useRef<GridBitmap | null>(null);
  const analyzerRef = useRef<SpectrumAnalyzer | null>(null);
  const dbRef = useRef<Float32Array | null>(null);
  const settingsRef = useRef({ view, eyeComponent, symbolRate, decimate });
  useLayoutEffect(() => {
    settingsRef.current = { view, eyeComponent, symbolRate, decimate };
  });

  // The grid accumulates across bursts, so anything that changes what is being plotted has to
  // empty it — otherwise the previous view's shape stays smeared under the new one.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the clear is the effect
  useEffect(() => {
    const grid = gridRef.current;
    if (grid !== null) {
      clearBasebandGrid(grid);
      bitmapRef.current?.invalidate();
    }
  }, [view, eyeComponent, symbolRate, decimate]);

  useEffect(() => {
    let seen = 0;
    return iqHub.subscribe(deviceSet, channel.id, (burst) => {
      frameRef.current = burst;
      const {
        view: mode,
        eyeComponent: rail,
        symbolRate: rate,
        decimate: sparse,
      } = settingsRef.current;
      if (mode !== "spectrum") {
        const grid = gridRef.current ?? createBasebandGrid(GRID, GRID);
        gridRef.current = grid;
        decayBasebandGrid(grid);
        bitmapRef.current?.invalidate();
        const period = samplesPerSymbol(burst.sampleRate, rate);
        if (mode === "constellation") {
          addConstellation(
            grid,
            burst.samples,
            peakMagnitude(burst.samples),
            sparse ? Math.round(period) : 1,
          );
        } else {
          addEye(grid, burst.samples, period, rail, eyeScale(burst.samples, rail));
        }
      }
      // The readout is text, and a burst arrives twenty times a second; the canvas redraws every
      // animation frame regardless, so this only paces the labels.
      seen += 1;
      if (seen === 1 || seen % 10 === 0) {
        setFrame(burst);
      }
    });
  }, [deviceSet, channel.id]);

  useEffect(() => {
    let raf = 0;
    const loop = () => {
      draw(
        canvasRef.current,
        frameRef.current,
        settingsRef.current.view,
        gridRef.current,
        bitmapRef,
        colormap,
        analyzerRef,
        dbRef,
      );
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [colormap]);

  const nyquist = frame === null ? Number.POSITIVE_INFINITY : frame.sampleRate / 2;
  const period = frame === null ? 0 : samplesPerSymbol(frame.sampleRate, symbolRate);

  return (
    <div className="relative flex h-full min-h-0 flex-col overflow-hidden bg-plot-bg">
      <canvas ref={canvasRef} className="h-full w-full min-h-0 flex-1" />

      <div className="pointer-events-none absolute inset-0 flex flex-col justify-between p-1.5">
        <span className="legend self-end text-right whitespace-pre text-plot-ink-dim">
          {frame === null ? "" : formatReadout(frame, view, period)}
        </span>
        <div
          data-plot-chrome
          className="pointer-events-auto flex max-w-full flex-wrap items-center gap-1 self-start rounded-[3px] bg-plot-bg/85 p-0.5"
        >
          {BASEBAND_VIEWS.map((name) => (
            <Button
              key={name}
              type="button"
              className={plotButton(view === name)}
              aria-pressed={view === name}
              onClick={() => setView(name)}
            >
              {name}
            </Button>
          ))}
          {view === "eye" && (
            <Popover
              label={eyeComponent}
              triggerClass={plotButton(false)}
              width="w-auto min-w-[var(--anchor-width)]"
              padded={false}
            >
              {(close) => (
                <div className="flex flex-col p-0.5">
                  {EYE_COMPONENTS.map((name) => (
                    <Button
                      key={name}
                      type="button"
                      className={`${segment(name === eyeComponent)} justify-start`}
                      onClick={() => {
                        setEyeComponent(name);
                        close();
                      }}
                    >
                      {name}
                    </Button>
                  ))}
                </div>
              )}
            </Popover>
          )}
          {view === "constellation" && (
            <Button
              type="button"
              className={plotButton(decimate)}
              aria-pressed={decimate}
              onClick={() => setDecimate(!decimate)}
            >
              symbols
            </Button>
          )}
          {(view === "eye" || (view === "constellation" && decimate)) && (
            <>
              <NumberField
                label="Symbol rate"
                className="w-20"
                value={symbolRate}
                min={MIN_SYMBOL_RATE}
                max={nyquist}
                step={100}
                onCommit={setSymbolRate}
              />
              <span className="legend text-plot-ink-dim">Bd</span>
            </>
          )}
        </div>
      </div>

      <span className="pointer-events-none absolute inset-x-0 top-0 p-1.5 text-plot-ink-dim legend">
        {label}
      </span>

      {frame === null && (
        <p className="pointer-events-none absolute inset-0 flex items-center justify-center pb-12 text-sm text-plot-ink-dim">
          Waiting for the first burst…
        </p>
      )}
    </div>
  );
}

function formatReadout(frame: IqFrame, view: BasebandView, period: number): string {
  const rate =
    frame.sampleRate >= 1e6
      ? `${(frame.sampleRate / 1e6).toFixed(3)} MSa/s`
      : `${(frame.sampleRate / 1e3).toFixed(1)} kSa/s`;
  const centre = `${(frame.centerHz / 1e6).toFixed(4)} MHz`;
  if (view === "spectrum") {
    return `${centre}   ${rate}`;
  }
  return `${centre}   ${rate}   ${period.toFixed(2)} Sa/sym`;
}

/** Repaint whichever view is showing. Sized in CSS pixels like the trace panel above it. */
function draw(
  canvas: HTMLCanvasElement | null,
  frame: IqFrame | null,
  view: BasebandView,
  grid: BasebandGrid | null,
  bitmapRef: { current: GridBitmap | null },
  colormap: Colormap,
  analyzerRef: { current: SpectrumAnalyzer | null },
  dbRef: { current: Float32Array | null },
): void {
  if (canvas === null || frame === null) {
    return;
  }
  if (view === "spectrum") {
    const analyzer = analyzerRef.current ?? new SpectrumAnalyzer(FFT_SIZE);
    analyzerRef.current = analyzer;
    const db = analyzer.powerDb(frame.samples, dbRef.current ?? new Float32Array(FFT_SIZE));
    dbRef.current = db;
    let peak = Number.NEGATIVE_INFINITY;
    for (const value of db) {
      if (value > peak) {
        peak = value;
      }
    }
    // Rounded to 10 dB so the axis labels stop dancing between bursts.
    const top = Math.ceil(peak / 10) * 10;
    drawPlot(canvas, {
      frame: { centerHz: frame.centerHz, spanHz: frame.sampleRate, db },
      view: FULL_VIEW,
      window: { min: top - SPECTRUM_RANGE_DB, max: top },
      traces: [],
      density: null,
    });
    return;
  }
  drawScatter(canvas, grid, bitmapRef, colormap, view);
}

function drawScatter(
  canvas: HTMLCanvasElement,
  grid: BasebandGrid | null,
  bitmapRef: { current: GridBitmap | null },
  colormap: Colormap,
  view: BasebandView,
): void {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0 || grid === null) {
    return;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return;
  }
  ctx.clearRect(0, 0, width, height);

  // A constellation's axes are the same quantity, so it is drawn square and centred; an eye's are
  // not, and it fills the panel.
  const side = view === "constellation" ? Math.min(width, height) : 0;
  const box =
    side > 0
      ? { x: (width - side) / 2, y: (height - side) / 2, w: side, h: side }
      : { x: 0, y: 0, w: width, h: height };

  ctx.strokeStyle = token("plot-grid");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(box.x, Math.round(box.y + box.h / 2) + 0.5);
  ctx.lineTo(box.x + box.w, Math.round(box.y + box.h / 2) + 0.5);
  if (view === "constellation") {
    ctx.moveTo(Math.round(box.x + box.w / 2) + 0.5, box.y);
    ctx.lineTo(Math.round(box.x + box.w / 2) + 0.5, box.y + box.h);
    ctx.stroke();
    // The unit circle: a PSK constellation sits on it, and an amplitude error shows as a ring
    // that is not this one.
    ctx.beginPath();
    ctx.arc(box.x + box.w / 2, box.y + box.h / 2, box.w / 2, 0, Math.PI * 2);
  }
  ctx.stroke();

  const bitmap = bitmapRef.current ?? new GridBitmap(grid.width, grid.height);
  bitmapRef.current = bitmap;
  bitmap.blit(ctx, box, (out) => recolour(grid, colormap, out));
}

/** Colour the density grid into RGBA bytes. Cells at zero come out transparent, so the axes and
 * the unit circle drawn underneath show through where nothing has been plotted. */
function recolour(grid: BasebandGrid, colormap: Colormap, out: Uint8ClampedArray): void {
  const lut = colormapLut(colormap);
  for (let i = 0; i < grid.cells.length; i++) {
    const value = grid.cells[i] ?? 0;
    const at = i * 4;
    if (value <= 0) {
      out[at + 3] = 0;
      continue;
    }
    const entry = Math.min(255, Math.round(value * 255)) * 3;
    out[at] = lut[entry] ?? 0;
    out[at + 1] = lut[entry + 1] ?? 0;
    out[at + 2] = lut[entry + 2] ?? 0;
    out[at + 3] = Math.min(255, Math.round(40 + value * 215));
  }
}
