// The scope face: trace + waterfall for whatever radio its `iq` wire comes from — CANVAS §1's
// "the WebGL plot, one component, patched anywhere". Binary frames bypass React state and go
// straight to the canvases (PLAN §10: high-rate streams never touch TanStack Query); only the
// readout's slow-moving metadata is state.
//
// The plot is the instrument, so it owns its gestures — but only once its node is the active one
// (`useFaceActive`): wheel zooms about the cursor, a drag pans, a click tunes, and a marker drag
// moves a channel. Until then the camera keeps both, and a click on the plot only brings the face
// forward, which is also what stops a stray click on a scope nobody selected from retuning a
// running radio.
import {
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useEffect,
  useRef,
  useState,
} from "react";
import { plotButton, segment } from "../../components/controls";
import { formatSignedKhz } from "../../components/format";
import { Popover } from "../../components/Popover";
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
} from "../../components/spectrumView";
import { pixelRatio, zoomOf } from "../../gl/raster";
import { attachWaterfall, COLORMAPS, type Colormap, type WaterfallView } from "../../gl/waterfall";
import type { SpectrumFrame } from "../../lib/frame";
import { spectrumHub } from "../../lib/spectrum";
import { token } from "../../lib/tokens";
import type { ChannelInfo, ChannelParams, DeviceSet, PatchNode } from "../../lib/types";
import { useChannelPatch } from "../../lib/useChannelPatch";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { channelNodesOf, iqSourceOf } from "../binding";
import { deviceSetOf, useWorkspaceContext } from "../context";
import { tuneDelta } from "./DeviceFace";
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";

/** Below this a pointer gesture is a click, not a pan (DESIGN.md §9). */
const DRAG_SLOP_PX = 4;
/** How close the pointer must be to a marker to grab it rather than pan the plot. */
const GRAB_PX = 10;
const COLORMAP_KEY = "sdrmm.colormap";
const TRACE_MIN = 0.15;
const TRACE_MAX = 0.75;
/** Rows the frequency axis reserves at the bottom of the trace canvas, in CSS pixels. */
const AXIS_H = 16;
/** Gridlines carry less ink than the data they sit behind (DESIGN.md §2). */
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

export function ScopeFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const source = iqSourceOf(workspace.graph, node.id);

  return (
    <NodeShell
      node={node}
      title="Scope"
      category="display"
      subtitle={set?.device.label}
      live={set !== null}
    >
      <FaceBody scroll={false}>
        {set === null ? (
          <FaceEmpty>
            {source !== null
              ? "The radio this scope watches is not attached. The wire is kept."
              : "Wire a device's IQ out to watch its spectrum."}
          </FaceEmpty>
        ) : (
          // Keyed on the radio *and* the lane its wire names: another device set — or another
          // stream of the same one — is another span, and markers, max hold and the waterfall's
          // history must never be carried across two of them.
          <Spectrum
            key={`${set.id}:${source?.stream ?? 0}`}
            node={node}
            set={set}
            stream={source?.stream ?? 0}
          />
        )}
      </FaceBody>
    </NodeShell>
  );
}

function Spectrum({ node, set, stream }: { node: PatchNode; set: DeviceSet; stream: number }) {
  const workspace = useWorkspaceContext();
  const { applyPatch } = useDevicePatch();
  const { applyEdit } = useChannelPatch();
  const active = useFaceActive();

  const plotRef = useRef<HTMLDivElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const traceRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallView | null>(null);
  // The hub kept the last frame that arrived while this face did not exist (lib/spectrum.ts).
  // Read at first render rather than in an effect: the trace and the readout have to be there in
  // the rack's first paint, or the switch still shows the blank the history exists to remove.
  // `Spectrum` is keyed by lane, so a different lane is a different mount reading its own.
  const frameRef = useRef<SpectrumFrame | null>(spectrumHub.latest(set.id, stream));
  const holdRef = useRef<Uint8Array | null>(null);
  const gestureRef = useRef<Gesture | null>(null);

  const [meta, setMeta] = useState<FrameMeta | null>(() =>
    frameRef.current === null ? null : metaOf(frameRef.current),
  );
  const [glError, setGlError] = useState<string | null>(null);
  const [view, setView] = useState<SpectrumView>(FULL_VIEW);
  const [hold, setHold] = useState(false);
  const [colormap, setColormap] = useState<Colormap>(readColormap);
  const [traceFraction, setTraceFraction] = useState(0.32);
  const [preview, setPreview] = useState<{ channel: number; offsetHz: number } | null>(null);
  const [panning, setPanning] = useState(false);
  const [picked, setPicked] = useState<number | null>(null);

  // Read inside the render loop and the frame handler, neither of which may be rebuilt per
  // frame or per state change.
  const viewRef = useRef(view);
  viewRef.current = view;
  const holdRequested = useRef(hold);
  holdRequested.current = hold;

  // Engine channel id → the node whose face tunes it. Built by following the wires rather than
  // by matching ids, because a channel id is only unique within its device set.
  const faces = new Map<number, string>();
  const deviceNode = iqSourceOf(workspace.graph, node.id)?.source;
  if (deviceNode !== undefined) {
    for (const { node: channelNode } of channelNodesOf(workspace.graph, deviceNode)) {
      const channel = workspace.channels.get(channelNode.id);
      if (channel !== undefined) {
        faces.set(channel.id, channelNode.id);
      }
    }
  }

  // The channel a click tunes. The workspace's selection wins while it names a channel on this
  // radio; otherwise the last marker picked here stands — clicking the plot also selects the
  // scope node, and that must not silently switch the click from the channel to the receiver.
  const workspaceChannel = [...faces].find(([, id]) => id === workspace.selected)?.[0] ?? null;
  const selectedChannel =
    workspaceChannel ?? (set.channels.some((channel) => channel.id === picked) ? picked : null);

  const selectChannel = (channel: number): void => {
    setPicked(channel);
    const face = faces.get(channel);
    if (face !== undefined) {
      workspace.select(face);
    }
  };

  // The frame's centre is the *lane's* (the readout binds to it directly), so a click computed
  // from it must retune the same lane: on a radio whose streams tune apart, a radio-wide patch
  // here would move every unoverridden lane — and not this one, if it holds an override.
  const tuneCenter = (hz: number): void =>
    applyPatch(set.id, tuneDelta(set.capabilities, stream, hz));
  const tuneChannel = (channel: number, offsetHz: number): void =>
    applyEdit(set.id, channel, { offset_hz: offsetHz });

  useEffect(() => {
    const canvas = waterfallRef.current;
    if (canvas === null) {
      return;
    }
    let renderer: WaterfallView;
    try {
      // `setGlError` is also the renderer's channel for what goes wrong later — a GPU reset takes
      // the shared context out from under every scope at once — and for clearing it again once
      // the plot is drawing from a rebuilt one.
      renderer = attachWaterfall(canvas, setGlError);
    } catch (error) {
      // No WebGL2, or a driver that refuses the shader. The waterfall is the centerpiece, but
      // throwing out of a node's mount takes the whole canvas down with it; the trace, the
      // controls and every other face still work without it.
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
    rendererRef.current?.setWindow(view.start, viewWidth(view));
  }, [view]);

  // The hub refcounts the subscription and re-sends it after a reconnect, so two scopes on one
  // radio cost one stream and neither can stop the other's. The lane is the one this scope's
  // own IQ wire names, not always the radio's first.
  //
  // It also kept the lane's recent rows while this face did not exist, and switching between the
  // patch and the rack remounts every face: the plot opens on the waterfall the operator was
  // already reading, rather than blanking and filling in again from the next frame. Seeded here
  // and not at render, unlike the frame above, because the renderer is the previous effect's.
  useEffect(() => {
    const past = spectrumHub.history(set.id, stream);
    rendererRef.current?.seed(past.rows, past.count, past.bins);
    let count = 0;
    return spectrumHub.subscribe(set.id, stream, (frame) => {
      frameRef.current = frame;
      rendererRef.current?.pushRow(frame.bins);
      accumulateHold(holdRef, frame.bins, holdRequested.current);
      // Metadata seeds from the first frame then throttles to ~4 Hz; the canvases redraw every
      // frame regardless, so this only paces the text.
      count += 1;
      if (count === 1 || count % 8 === 0) {
        setMeta(metaOf(frame));
      }
    });
  }, [set.id, stream]);

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

  // React marks its delegated wheel listener passive, so zoom has to be bound natively or the
  // page scrolls underneath the gesture. Bound only while this face is the active one: otherwise
  // one wheel notch would zoom the spectrum and pan the patch at the same time.
  useEffect(() => {
    const plot = plotRef.current;
    if (plot === null || !active) {
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
  }, [active]);

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
    // An inactive face is brought forward by the click and nothing else: the pointer belongs to
    // the camera there, and a plot that tuned on first contact would retune a radio the operator
    // was only reaching past.
    if (!active || event.button !== 0 || plotRef.current === null || spanHz <= 0) {
      return;
    }
    const rect = plotRef.current.getBoundingClientRect();
    const at = pointerFraction(event.clientX);
    const grabbed = markerAt(set.channels, view, spanHz, at, GRAB_PX / rect.width);
    if (grabbed !== null) {
      selectChannel(grabbed.id);
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
    // The rendered width, not the laid-out one: `clientX` is screen pixels, and React Flow
    // magnifies the node with a CSS transform, so dividing by `clientWidth` would pan the
    // spectrum by the canvas zoom factor rather than by however far the pointer moved.
    const rect = plotRef.current?.getBoundingClientRect();
    setView(panView(gesture.view, (gesture.pointerX - event.clientX) / (rect?.width || 1)));
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
        tuneChannel(gesture.channel, preview.offsetHz);
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
      tuneChannel(selectedChannel, offsetHz);
    } else {
      tuneCenter(meta.centerHz + offsetHz);
    }
  };

  return (
    <div
      ref={plotRef}
      className={`relative flex h-full min-h-0 flex-col overflow-hidden bg-plot-bg ${
        active ? "nodrag nopan nowheel touch-none cursor-crosshair" : "cursor-default"
      } ${panning ? "!cursor-grabbing" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={(event) => {
        if (!active || meta === null) {
          return;
        }
        const at = pointerFraction(event.clientX);
        tuneCenter(Math.round(meta.centerHz + spanToOffset(viewToSpan(view, at), meta.spanHz)));
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
          the height it was last given and overflow the node. */}
      <canvas ref={waterfallRef} className="w-full min-h-0 flex-1" />

      {meta !== null && (
        <Markers
          channels={set.channels}
          view={view}
          spanHz={meta.spanHz}
          selected={selectedChannel}
          preview={preview}
          onSelect={selectChannel}
        />
      )}

      <div className="pointer-events-none absolute inset-0 flex flex-col justify-between p-1.5">
        <span className="legend self-end text-right whitespace-pre text-plot-ink-dim">
          {meta !== null && `${formatCentre(meta, view)}${formatRange(meta)}`}
        </span>
        {/* Bottom-left: the only corner of the plot no data occupies, so the toolbar costs the
            trace nothing. */}
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
          {!isFullView(view) && (
            <button type="button" className={plotButton(false)} onClick={() => setView(FULL_VIEW)}>
              {(1 / viewWidth(view)).toFixed(1)}× · reset
            </button>
          )}
        </div>
      </div>

      {glError !== null && (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 flex justify-center p-2">
          <span className="rounded-[3px] border border-danger bg-bg/90 px-2 py-1 font-mono text-xs text-danger">
            waterfall unavailable: {glError}
          </span>
        </div>
      )}

      {meta === null && (
        <p className="pointer-events-none absolute inset-0 flex items-center justify-center pb-12 text-sm text-plot-ink-dim">
          Waiting for the first frame…
        </p>
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
  plotRef: RefObject<HTMLDivElement | null>;
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
  onSelect: (channel: number) => void;
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
              // The hit strip is invisible and wide; the drawn line stays 1px, because ink and
              // target size are different budgets.
              className="pointer-events-auto absolute inset-y-0 w-5 -translate-x-1/2 cursor-ew-resize"
              style={{ left: `${at * 100}%` }}
              // React Flow selects the node a click landed in, which would take the selection
              // straight back off the channel this click just put it on.
              onClick={(event) => {
                event.stopPropagation();
                onSelect(channel.id);
              }}
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

function metaOf(frame: SpectrumFrame): FrameMeta {
  return {
    centerHz: frame.centerHz,
    spanHz: frame.spanHz,
    dbMin: frame.dbMin,
    dbMax: frame.dbMax,
  };
}

function accumulateHold(
  ref: RefObject<Uint8Array | null>,
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
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0) {
    return;
  }
  // Same rule as the waterfall (CANVAS §7): React Flow magnifies the node with a CSS transform,
  // so the backing store follows the zoom or the trace is a stretched bitmap.
  const rect = canvas.getBoundingClientRect();
  const ratio = pixelRatio(window.devicePixelRatio, zoomOf(rect.width, width));
  const w = Math.round(width * ratio);
  const h = Math.round(height * ratio);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return;
  }
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
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
