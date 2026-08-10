// Spectrum trace + WebGL2 waterfall for one device set (DESIGN.md §7). Binary frames bypass
// React state and go straight to the canvases (PLAN §10: high-rate streams never touch TanStack
// Query); only the readout's slow-moving metadata is state.
//
// The plot is the instrument, so it owns its own gestures: wheel zooms about the cursor, a drag
// pans, a click tunes, and a marker drag moves a channel.
import {
  type ReactNode,
  type PointerEvent as ReactPointerEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import { COLORMAPS, type Colormap, WaterfallRenderer } from "../gl/waterfall";
import type { SpectrumFrame } from "../lib/frame";
import { token } from "../lib/tokens";
import type { ChannelInfo, ChannelParams } from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import { plotButton, segment } from "./controls";
import { formatSignedKhz } from "./format";
import { Popover } from "./Popover";
import {
  decibelTicks,
  FULL_VIEW,
  frequencyTicks,
  isFullView,
  offsetToSpan,
  panView,
  type SpectrumView,
  spanToOffset,
  spanToView,
  viewToSpan,
  viewWidth,
  zoomView,
} from "./spectrumView";

const BINS = 1024;
const FPS = 30;
/** Below this a pointer gesture is a click, not a pan (DESIGN.md §7). */
const DRAG_SLOP_PX = 4;
/** How close the pointer must be to a marker to grab it rather than pan the plot. */
const GRAB_PX = 10;
const COLORMAP_KEY = "sdrmm.colormap";
const TRACE_MIN = 0.15;
const TRACE_MAX = 0.75;
/** Rows the frequency axis reserves at the bottom of the trace canvas, in CSS pixels. */
const AXIS_H = 16;
/** Gridlines carry less ink than the data they sit behind (DESIGN.md §7). */
const GRID_ALPHA = 0.16;

interface FrameMeta {
  centerHz: number;
  spanHz: number;
  dbMin: number;
  dbMax: number;
}

/** The gesture in flight. Held in a ref: a pan updates the view sixty times a second and must
 * not also re-render on its own bookkeeping. */
interface Gesture {
  pointerX: number;
  at: number;
  view: SpectrumView;
  channel: number | null;
  moved: boolean;
}

export function SpectrumDisplay({
  socket,
  deviceSet,
  connected,
  channels,
  selectedChannel,
  onSelectChannel,
  onTuneCenter,
  onTuneChannel,
  empty,
}: {
  socket: SdrSocket;
  deviceSet: number | null;
  connected: boolean;
  channels: readonly ChannelInfo[];
  selectedChannel: number | null;
  onSelectChannel: (ch: number) => void;
  onTuneCenter: (hz: number) => void;
  onTuneChannel: (ch: number, offsetHz: number) => void;
  /** Shown over the plot when no radio is open — the place a station is started from, so it
   * sits where the operator is already looking. */
  empty: ReactNode;
}) {
  const plotRef = useRef<HTMLDivElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const traceRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallRenderer | null>(null);
  const frameRef = useRef<SpectrumFrame | null>(null);
  const holdRef = useRef<Uint8Array | null>(null);
  const gestureRef = useRef<Gesture | null>(null);

  const [meta, setMeta] = useState<FrameMeta | null>(null);
  const [glError, setGlError] = useState<string | null>(null);
  const [view, setView] = useState<SpectrumView>(FULL_VIEW);
  const [hold, setHold] = useState(false);
  const [colormap, setColormap] = useState<Colormap>(readColormap);
  const [traceFraction, setTraceFraction] = useState(0.32);
  const [preview, setPreview] = useState<{ channel: number; offsetHz: number } | null>(null);
  const [panning, setPanning] = useState(false);

  // Read inside the render loop and the frame handler, neither of which may be rebuilt per
  // frame or per state change.
  const viewRef = useRef(view);
  viewRef.current = view;
  const holdRequested = useRef(hold);
  holdRequested.current = hold;

  useEffect(() => {
    const canvas = waterfallRef.current;
    if (!canvas) {
      return;
    }
    let renderer: WaterfallRenderer;
    try {
      renderer = new WaterfallRenderer(canvas);
    } catch (error) {
      // No WebGL2, a driver that refuses the shader, a lost context: the waterfall is the
      // centerpiece, but throwing out of a dock panel's mount takes the whole UI down with it.
      // The trace, the controls and every other panel still work without it.
      setGlError(error instanceof Error ? error.message : String(error));
      return;
    }
    rendererRef.current = renderer;
    return () => {
      renderer.dispose();
      rendererRef.current = null;
    };
  }, []);

  useEffect(() => {
    rendererRef.current?.setColormap(colormap);
  }, [colormap]);

  useEffect(() => {
    rendererRef.current?.setView(view.start, viewWidth(view));
  }, [view]);

  useEffect(() => {
    // A device-set switch invalidates everything cached from frames: markers and the readout
    // must never be placed by the previous set's span, and a max-hold would mix two radios.
    setMeta(null);
    setView(FULL_VIEW);
    frameRef.current = null;
    holdRef.current = null;
    let count = 0;
    socket.onSpectrum = (frame: SpectrumFrame) => {
      // Spectrum stream ids are device-set ids; drop late frames from a previous set.
      if (frame.streamId !== deviceSet) {
        return;
      }
      frameRef.current = frame;
      rendererRef.current?.pushRow(frame.bins);
      accumulateHold(holdRef, frame.bins, holdRequested.current);
      // Metadata seeds from the first frame then throttles to ~4 Hz; the canvases redraw every
      // frame regardless, so this only paces the text.
      count += 1;
      if (count === 1 || count % 8 === 0) {
        setMeta({
          centerHz: frame.centerHz,
          spanHz: frame.spanHz,
          dbMin: frame.dbMin,
          dbMax: frame.dbMax,
        });
      }
    };
    return () => {
      socket.onSpectrum = () => {};
    };
  }, [socket, deviceSet]);

  // Drawing is driven by animation frames rather than by arriving data: a pan or a zoom must
  // repaint the trace even while the radio is between frames.
  useEffect(() => {
    let raf = 0;
    const loop = () => {
      drawTrace(
        traceRef.current,
        frameRef.current,
        viewRef.current,
        holdRequested.current ? holdRef.current : null,
      );
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);

  // Subscribe only once the socket is actually open, and re-subscribe whenever it reconnects
  // (`connected` cycles false→true). `send()` drops commands while not OPEN, so gating on
  // `connected` avoids both the initial CONNECTING race and a permanently frozen stream after a
  // reconnect (the new server connection has no subscriptions).
  useEffect(() => {
    if (deviceSet === null || !connected) {
      return;
    }
    socket.send({
      type: "SubscribeSpectrum",
      data: { device_set: deviceSet, fps: FPS, bins: BINS },
    });
    return () => {
      socket.send({ type: "UnsubscribeSpectrum", data: { device_set: deviceSet } });
    };
  }, [socket, deviceSet, connected]);

  // React marks its delegated wheel listener passive, so zoom has to be bound natively or the
  // dock panel scrolls underneath the gesture.
  useEffect(() => {
    const plot = plotRef.current;
    if (plot === null) {
      return;
    }
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = plot.getBoundingClientRect();
      const at = (event.clientX - rect.left) / rect.width;
      setView((current) => zoomView(current, at, event.deltaY < 0 ? 1.2 : 1 / 1.2));
    };
    plot.addEventListener("wheel", onWheel, { passive: false });
    return () => plot.removeEventListener("wheel", onWheel);
  }, []);

  const chooseColormap = (next: Colormap): void => {
    setColormap(next);
    try {
      localStorage.setItem(COLORMAP_KEY, next);
    } catch {
      // A blocked store costs the preference on the next load, not this session.
    }
  };

  const spanHz = meta?.spanHz ?? 0;
  const pointerFraction = (clientX: number): number => {
    const rect = plotRef.current?.getBoundingClientRect();
    return rect === undefined || rect.width === 0 ? 0 : (clientX - rect.left) / rect.width;
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (event.button !== 0 || plotRef.current === null || spanHz <= 0) {
      return;
    }
    const rect = plotRef.current.getBoundingClientRect();
    const at = pointerFraction(event.clientX);
    const grabbed = markerAt(channels, view, spanHz, at, GRAB_PX / rect.width);
    if (grabbed !== null) {
      onSelectChannel(grabbed.id);
    }
    // Recorded before the capture is requested: the gesture must survive a pointer the browser
    // refuses to capture, or the release would find nothing to act on.
    gestureRef.current = {
      pointerX: event.clientX,
      at,
      view,
      channel: grabbed?.id ?? null,
      moved: false,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>): void => {
    const gesture = gestureRef.current;
    if (gesture === null) {
      return;
    }
    if (!gesture.moved && Math.abs(event.clientX - gesture.pointerX) < DRAG_SLOP_PX) {
      return;
    }
    gesture.moved = true;
    const at = pointerFraction(event.clientX);
    if (gesture.channel !== null) {
      // The offset is previewed while dragging and committed on release: a PATCH per pointer
      // event would flood the server and fight the state it echoes back.
      setPreview({
        channel: gesture.channel,
        offsetHz: Math.round(spanToOffset(viewToSpan(gesture.view, at), spanHz)),
      });
      return;
    }
    setPanning(true);
    setView(
      panView(
        gesture.view,
        (gesture.pointerX - event.clientX) / (plotRef.current?.clientWidth || 1),
      ),
    );
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>): void => {
    const gesture = gestureRef.current;
    gestureRef.current = null;
    setPanning(false);
    setPreview(null);
    if (gesture === null) {
      return;
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (gesture.moved) {
      if (gesture.channel !== null && preview !== null) {
        onTuneChannel(gesture.channel, preview.offsetHz);
      }
      return;
    }
    // A click *on* a marker only selects it — `onPointerDown` already did that, and tuning it
    // to the pointer would move the channel by up to the grab tolerance every time it is picked.
    if (meta === null || gesture.channel !== null) {
      return;
    }
    // Elsewhere, a click tunes what the operator is listening to; with nothing selected there
    // is only the radio itself to move.
    const offsetHz = Math.round(spanToOffset(viewToSpan(gesture.view, gesture.at), meta.spanHz));
    if (selectedChannel !== null) {
      onTuneChannel(selectedChannel, offsetHz);
    } else {
      onTuneCenter(meta.centerHz + offsetHz);
    }
  };

  return (
    <div
      ref={plotRef}
      className={`relative flex h-full min-h-0 flex-col overflow-hidden bg-plot-bg touch-none ${
        panning ? "cursor-grabbing" : "cursor-crosshair"
      }`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={(event) => {
        if (meta === null) {
          return;
        }
        const at = pointerFraction(event.clientX);
        onTuneCenter(Math.round(meta.centerHz + spanToOffset(viewToSpan(view, at), meta.spanHz)));
        setView(FULL_VIEW);
      }}
    >
      <canvas
        ref={traceRef}
        className="w-full shrink-0"
        style={{ height: `${traceFraction * 100}%` }}
      />
      <Divider fraction={traceFraction} onFraction={setTraceFraction} plotRef={plotRef} />
      {/* `min-h-0` is load-bearing: a canvas has an intrinsic size from its backing store, and a
          flex item defaults to `min-height: auto`, so the waterfall would refuse to shrink below
          the height it was last given and overflow its dock panel. */}
      <canvas ref={waterfallRef} className="w-full min-h-0 flex-1" />

      {meta !== null && deviceSet !== null && (
        <Markers
          channels={channels}
          view={view}
          spanHz={meta.spanHz}
          selected={selectedChannel}
          preview={preview}
          onSelect={onSelectChannel}
        />
      )}

      {deviceSet !== null && (
        <div className="pointer-events-none absolute inset-0 flex flex-col justify-between p-1.5">
          <span className="legend self-end text-right whitespace-pre text-plot-ink-dim">
            {meta !== null && (
              <>
                {formatCentre(meta, view)}
                {/* The dB range is the first thing to go when the bar is narrower than the
                    numbers: the frequency is what the operator is reading. */}
                <span className="max-md:hidden">{formatRange(meta)}</span>
              </>
            )}
          </span>
          {/* Bottom-left: the only corner of the plot no data occupies, so the toolbar costs
              the trace nothing. */}
          <div className="pointer-events-auto flex items-center gap-1 self-start">
            <Popover label={colormap} triggerClass={plotButton(false)} width="w-36">
              {(close) => (
                <div className="flex flex-col gap-0.5">
                  {COLORMAPS.map((name) => (
                    <button
                      key={name}
                      type="button"
                      className={`${segment(name === colormap)} justify-start`}
                      onClick={() => {
                        chooseColormap(name);
                        close();
                      }}
                    >
                      {name}
                    </button>
                  ))}
                </div>
              )}
            </Popover>
            <button
              type="button"
              className={plotButton(hold)}
              aria-pressed={hold}
              onClick={() => {
                holdRef.current = null;
                setHold(!hold);
              }}
            >
              max hold
            </button>
            {/* A touch pointer has no wheel, so the zoom gesture needs buttons of its own. */}
            <span className="hidden items-center gap-1 pointer-coarse:flex">
              <button
                type="button"
                className={plotButton(false)}
                aria-label="Zoom out"
                onClick={() => setView((current) => zoomView(current, 0.5, 1 / 1.6))}
              >
                −
              </button>
              <button
                type="button"
                className={plotButton(false)}
                aria-label="Zoom in"
                onClick={() => setView((current) => zoomView(current, 0.5, 1.6))}
              >
                +
              </button>
            </span>
            {!isFullView(view) && (
              <button
                type="button"
                className={plotButton(false)}
                onClick={() => setView(FULL_VIEW)}
              >
                {(1 / viewWidth(view)).toFixed(1)}× · reset
              </button>
            )}
          </div>
        </div>
      )}

      {glError !== null && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 flex justify-center p-2">
          <span className="rounded-[3px] border border-danger bg-bg/90 px-2 py-1 font-mono text-xs text-danger">
            waterfall unavailable: {glError}
          </span>
        </div>
      )}

      {deviceSet === null ? (
        // The canvases stay mounted underneath so the GL context survives a radio being closed
        // and reopened; only the invitation is layered on top.
        <div className="absolute inset-0 flex items-center justify-center bg-plot-bg px-6 pb-12">
          {empty}
        </div>
      ) : (
        meta === null && (
          <p className="pointer-events-none absolute inset-0 flex items-center justify-center pb-12 text-sm text-plot-ink-dim">
            Waiting for the first frame…
          </p>
        )
      )}
    </div>
  );
}

/** The trace/waterfall split. A 9px grab strip around a 1px rule: a 1px-adjacent target needs
 * its hit surface extended, not its ink. */
function Divider({
  fraction,
  onFraction,
  plotRef,
}: {
  fraction: number;
  onFraction: (fraction: number) => void;
  plotRef: React.RefObject<HTMLDivElement | null>;
}) {
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label={`Trace and waterfall split, ${Math.round(fraction * 100)}% trace`}
      className="group relative z-10 -my-1 h-[9px] shrink-0 cursor-row-resize"
      onPointerDown={(event) => {
        event.stopPropagation();
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        if (!event.currentTarget.hasPointerCapture(event.pointerId) || plotRef.current === null) {
          return;
        }
        event.stopPropagation();
        const rect = plotRef.current.getBoundingClientRect();
        const next = (event.clientY - rect.top) / rect.height;
        onFraction(Math.min(TRACE_MAX, Math.max(TRACE_MIN, next)));
      }}
      onPointerUp={(event) => {
        event.stopPropagation();
        event.currentTarget.releasePointerCapture(event.pointerId);
      }}
    >
      <span
        aria-hidden
        className="absolute inset-x-0 top-1 h-px bg-plot-ink-dim/25 group-hover:bg-accent"
      />
    </div>
  );
}

function Markers({
  channels,
  view,
  spanHz,
  selected,
  preview,
  onSelect,
}: {
  channels: readonly ChannelInfo[];
  view: SpectrumView;
  spanHz: number;
  selected: number | null;
  preview: { channel: number; offsetHz: number } | null;
  onSelect: (ch: number) => void;
}) {
  const visible = spanHz * viewWidth(view);
  return (
    // Markers must not steal gestures from the plot: the layer is pointer-transparent and only
    // each marker's own hit strip takes the pointer.
    <div className="pointer-events-none absolute inset-0 overflow-hidden">
      {channels.map((channel) => {
        const offsetHz =
          preview?.channel === channel.id ? preview.offsetHz : (channel.settings.offset_hz ?? 0);
        const at = spanToView(view, offsetToSpan(offsetHz, spanHz));
        if (at < -0.02 || at > 1.02) {
          return null;
        }
        const active = channel.id === selected;
        const bandwidth = bandwidthHz(channel.settings.params);
        return (
          <div key={channel.id}>
            {bandwidth !== null && visible > 0 && (
              <span
                aria-hidden
                className={`absolute inset-y-0 -translate-x-1/2 ${active ? "bg-plot-ink/12" : "bg-plot-ink/6"}`}
                style={{ left: `${at * 100}%`, width: `${(bandwidth / visible) * 100}%` }}
              />
            )}
            <span
              aria-hidden
              className={`absolute inset-y-0 -translate-x-1/2 ${
                active ? "w-0.5 bg-plot-ink" : "w-px bg-plot-ink-dim"
              }`}
              style={{ left: `${at * 100}%` }}
            />
            <button
              type="button"
              // The hit strip is invisible and wide (40px on coarse pointers); the drawn line
              // stays 1px, because ink and target size are different budgets.
              className="pointer-events-auto absolute inset-y-0 w-5 -translate-x-1/2 cursor-ew-resize pointer-coarse:w-10"
              style={{ left: `${at * 100}%` }}
              onClick={() => onSelect(channel.id)}
              aria-label={`${channel.settings.params.type} channel at ${formatSignedKhz(offsetHz)} — drag to tune`}
            />
            <span
              aria-hidden
              className={`absolute top-7 -translate-x-1/2 rounded-[2px] border px-1 py-px font-mono text-[10px] whitespace-nowrap tabular-nums ${
                active ? "border-accent bg-bg text-accent" : "border-line bg-bg/85 text-ink-dim"
              }`}
              style={{ left: `${at * 100}%` }}
            >
              {channel.settings.params.type.toUpperCase()} {formatSignedKhz(offsetHz)}
            </span>
          </div>
        );
      })}
    </div>
  );
}

/** The passband a channel occupies, where its mode declares one. Modes without a bandwidth
 * (ADS-B, AIS) draw a line and no band rather than a made-up width. */
function bandwidthHz(params: ChannelParams): number | null {
  return "bandwidth_hz" in params.settings && typeof params.settings.bandwidth_hz === "number"
    ? params.settings.bandwidth_hz
    : null;
}

function markerAt(
  channels: readonly ChannelInfo[],
  view: SpectrumView,
  spanHz: number,
  at: number,
  tolerance: number,
): ChannelInfo | null {
  let best: ChannelInfo | null = null;
  let bestDistance = tolerance;
  for (const channel of channels) {
    const position = spanToView(view, offsetToSpan(channel.settings.offset_hz ?? 0, spanHz));
    const distance = Math.abs(position - at);
    if (distance <= bestDistance) {
      best = channel;
      bestDistance = distance;
    }
  }
  return best;
}

function accumulateHold(
  ref: React.RefObject<Uint8Array | null>,
  bins: Uint8Array,
  enabled: boolean,
): void {
  if (!enabled) {
    ref.current = null;
    return;
  }
  const held = ref.current;
  if (held === null || held.length !== bins.length) {
    ref.current = Uint8Array.from(bins);
    return;
  }
  for (let i = 0; i < bins.length; i++) {
    const value = bins[i] ?? 0;
    if (value > (held[i] ?? 0)) {
      held[i] = value;
    }
  }
}

function readColormap(): Colormap {
  try {
    const stored = localStorage.getItem(COLORMAP_KEY);
    return COLORMAPS.find((name) => name === stored) ?? "magma";
  } catch {
    return "magma";
  }
}

function formatCentre(meta: FrameMeta, view: SpectrumView): string {
  const visible = meta.spanHz * viewWidth(view);
  const centre = meta.centerHz + spanToOffset((view.start + view.end) / 2, meta.spanHz);
  const span =
    visible >= 1e6 ? `${(visible / 1e6).toFixed(3)} MHz` : `${(visible / 1e3).toFixed(1)} kHz`;
  return `${(centre / 1e6).toFixed(4)} MHz   ${span}`;
}

function formatRange(meta: FrameMeta): string {
  return `   ${meta.dbMin.toFixed(0)}…${meta.dbMax.toFixed(0)} dB`;
}

/** The trace, its grid and both axes. Drawn from the frame's own metadata, so the numbers on
 * screen are the ones the server measured. */
function drawTrace(
  canvas: HTMLCanvasElement | null,
  frame: SpectrumFrame | null,
  view: SpectrumView,
  hold: Uint8Array | null,
): void {
  if (!canvas) {
    return;
  }
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0) {
    return;
  }
  const w = Math.round(width * dpr);
  const h = Math.round(height * dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  if (frame === null || frame.bins.length < 2 || !(frame.dbMax > frame.dbMin)) {
    return;
  }
  const plotH = Math.max(1, height - AXIS_H);

  ctx.font = '10px ui-monospace, "SF Mono", Menlo, monospace';
  ctx.textBaseline = "middle";
  ctx.lineWidth = 1;

  // Gridlines stay lighter-weight than the data they sit behind. The alpha comes from
  // `globalAlpha`, not a translucent colour: canvas parses far fewer colour syntaxes than CSS
  // does, and a value it rejects silently leaves the previous, opaque one in place.
  // Half-pixel offsets so a 1px rule lands on one device row instead of straddling two.
  ctx.strokeStyle = token("plot-grid");
  ctx.fillStyle = token("plot-ink-dim");
  ctx.globalAlpha = GRID_ALPHA;
  for (const db of decibelTicks(frame.dbMin, frame.dbMax, 4)) {
    const y = Math.round(plotH * (1 - (db - frame.dbMin) / (frame.dbMax - frame.dbMin))) + 0.5;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(width, y);
    ctx.stroke();
    if (y > 12 && y < plotH - 4) {
      ctx.globalAlpha = 1;
      ctx.fillText(db.toFixed(0), 4, y - 7);
      ctx.globalAlpha = GRID_ALPHA;
    }
  }

  const visible = frame.spanHz * viewWidth(view);
  const ticks = frequencyTicks(
    frame.centerHz,
    frame.spanHz,
    view,
    Math.max(2, Math.floor(width / 110)),
  );
  ctx.textAlign = "center";
  for (const tick of ticks) {
    const x = Math.round(tick.at * width) + 0.5;
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, plotH);
    ctx.stroke();
    ctx.globalAlpha = 1;
    ctx.fillText(formatTick(tick.hz, visible), x, height - AXIS_H / 2);
    ctx.globalAlpha = GRID_ALPHA;
  }
  ctx.globalAlpha = 1;
  ctx.textAlign = "left";

  // Centre of the device passband — the frequency the dial is showing.
  const centerAt = spanToView(view, 0.5);
  if (centerAt >= 0 && centerAt <= 1) {
    const x = Math.round(centerAt * width) + 0.5;
    ctx.strokeStyle = token("plot-ink-dim");
    ctx.globalAlpha = 0.45;
    ctx.setLineDash([2, 4]);
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, plotH);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;
  }

  if (hold !== null) {
    ctx.strokeStyle = token("plot-hold");
    tracePath(ctx, hold, view, width, plotH);
    ctx.stroke();
  }

  ctx.strokeStyle = token("plot-trace");
  ctx.lineWidth = 1.25;
  ctx.lineJoin = "round";
  tracePath(ctx, frame.bins, view, width, plotH);
  ctx.stroke();
  // A fill under the trace gives the noise floor a body to read against without spending a
  // second colour on it.
  ctx.lineTo(width, plotH);
  ctx.lineTo(0, plotH);
  ctx.closePath();
  ctx.fillStyle = token("plot-trace");
  ctx.globalAlpha = 0.09;
  ctx.fill();
  ctx.globalAlpha = 1;
}

/** One screen column per pixel, taking the maximum of every bin that falls in it: decimating by
 * sampling would drop exactly the narrow carriers the display exists to show. */
function tracePath(
  ctx: CanvasRenderingContext2D,
  bins: Uint8Array,
  view: SpectrumView,
  width: number,
  height: number,
): void {
  const n = bins.length;
  const first = view.start * (n - 1);
  const last = view.end * (n - 1);
  ctx.beginPath();
  for (let x = 0; x < width; x++) {
    const from = first + ((last - first) * x) / width;
    const to = first + ((last - first) * (x + 1)) / width;
    const lo = Math.max(0, Math.floor(from));
    const hi = Math.min(n - 1, Math.max(lo, Math.ceil(to) - 1));
    let peak = 0;
    for (let i = lo; i <= hi; i++) {
      const value = bins[i] ?? 0;
      if (value > peak) {
        peak = value;
      }
    }
    const y = (1 - peak / 255) * height;
    if (x === 0) {
      ctx.moveTo(x, y);
    } else {
      ctx.lineTo(x, y);
    }
  }
}

/** Tick labels carry only the digits the current zoom can distinguish — six decimals on a 2 MHz
 * span is noise the reader has to parse past. */
function formatTick(hz: number, visibleHz: number): string {
  const decimals = visibleHz >= 5e6 ? 1 : visibleHz >= 5e5 ? 2 : visibleHz >= 5e4 ? 3 : 4;
  return (hz / 1e6).toFixed(decimals);
}
