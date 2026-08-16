import { useQuery } from "@tanstack/react-query";
import {
  Fragment,
  type ReactNode,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { Button } from "../../components/BaseControls";
import { identify, suggestedAt } from "../../components/bandPlan";
import { type Options, plotButton, segment, segmentSm } from "../../components/controls";
import { formatMhz, formatSignedKhz } from "../../components/format";
import { Popover } from "../../components/Popover";
import { Slider } from "../../components/Slider";
import { frozenAge, frozenCursor, frozenLength, frozenRow } from "../../components/spectrumFreeze";
import {
  accumulateTraces,
  type DbWindow,
  dequantize,
  frameWindow,
  requantize,
  requantizeHistory,
  TRACE_MODES,
  type TraceMode,
  type TraceState,
  traceOf,
} from "../../components/spectrumTraces";
import {
  clusterMarkers,
  FULL_VIEW,
  isFullView,
  labelWidth,
  offsetToSpan,
  panView,
  type SpectrumView,
  spanToOffset,
  spanToView,
  viewToSpan,
  viewWidth,
  zoomView,
} from "../../components/spectrumView";
import { rowsForHeight } from "../../gl/raster";
import {
  attachWaterfall,
  COLORMAPS,
  type Colormap,
  DEFAULT_COLORMAP,
  type WaterfallView,
} from "../../gl/waterfall";
import { bookmarksQuery } from "../../lib/api";
import type { SpectrumFrame } from "../../lib/frame";
import { SPECTRUM_HISTORY_ROWS, type SpectrumHistory, spectrumHub } from "../../lib/spectrum";
import type { Bookmark, ChannelInfo, ChannelParams, DeviceSet, PatchNode } from "../../lib/types";
import { useBandPlan } from "../../lib/useBandPlan";
import { useChannelPatch } from "../../lib/useChannelPatch";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { basebandSourceOf, channelNodesOf, hasWire, iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { addEdge, addNode, newNodeId, patchNode, streamPort } from "../graph";
import { useNodePlacement } from "../placement";
import { deviceSetOf } from "../workspaceDevice";
import { BandRuler } from "./BandRuler";
import { BasebandView } from "./BasebandView";
import { ChannelPicker } from "./ChannelPicker";
import { tuneDelta } from "./deviceNode";
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";
import { ScopeMenu, type ScopeMenuAt } from "./ScopeMenu";
import {
  bookmarkDraft,
  channelTypeAt,
  pickAt,
  type ScopePick,
  type ScopeSource,
  scopeSource,
  takeCreationTune,
  tuneOnCreate,
} from "./scopePick";
import { DensityLayer, drawPlot, type PlotFrame, type PlotTrace } from "./scopePlot";

const DRAG_SLOP_PX = 4;
const GRAB_PX = 10;
const COLORMAP_KEY = "sdrmm.colormap";
const TRACE_MIN = 0.15;
const TRACE_MAX = 0.75;
const LABEL_TOP_PX = 28;
const EMPTY_WINDOW: DbWindow = { min: -100, max: -20 };

interface FrameMeta {
  centerHz: number;
  spanHz: number;
  dbMin: number;
  dbMax: number;
}

interface Gesture {
  pointerX: number;
  at: number;
  view: SpectrumView;
  channel: number | null;
  moved: boolean;
}

const SOURCES: Options<ScopeSource> = [
  { value: "iq", label: "IQ" },
  { value: "baseband", label: "Base" },
];

export function ScopeFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const source = iqSourceOf(workspace.graph, node.id);
  const tap = basebandSourceOf(workspace.graph, node.id, workspace.devices, workspace.channels);
  const [colormap] = useState<Colormap>(readColormap);
  const [chosen, setChosen] = useState<ScopeSource>("iq");
  const shown = scopeSource(chosen, source !== null, tap !== null);

  const actions =
    source !== null && tap !== null ? (
      <span className="flex items-center" role="group" aria-label="Scope source">
        {SOURCES.map((option) => (
          <Button
            key={option.value}
            type="button"
            className={segmentSm(shown === option.value)}
            aria-pressed={shown === option.value}
            onClick={() => setChosen(option.value)}
          >
            {option.label}
          </Button>
        ))}
      </span>
    ) : undefined;

  if (shown === "baseband" && tap !== null) {
    return (
      <NodeShell
        node={node}
        title="Scope"
        category="display"
        subtitle={`${tap.channel.settings.params.type} baseband`}
        live
        actions={actions}
      >
        <FaceBody scroll={false}>
          <BasebandView
            key={`${tap.deviceSet}:${tap.channel.id}`}
            deviceSet={tap.deviceSet}
            channel={tap.channel}
            colormap={colormap}
            label={
              workspace.devices.get(iqSourceOf(workspace.graph, tap.node)?.source ?? "")?.device
                .label
            }
          />
        </FaceBody>
      </NodeShell>
    );
  }

  return (
    <NodeShell
      node={node}
      title="Scope"
      category="display"
      subtitle={set?.device.label}
      live={set !== null}
      actions={actions}
    >
      <FaceBody scroll={false}>
        {set === null ? (
          <FaceEmpty>
            {source !== null
              ? "No spectrum: the radio this scope watches is not connected. Plug it in and the trace comes back."
              : hasWire(workspace.graph, node.id, "baseband")
                ? "No baseband: the channel this scope taps is not running. Start it from its node."
                : "Wire a device's IQ out to watch its spectrum, or a channel's baseband out to watch one channel."}
          </FaceEmpty>
        ) : (
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
  const placeNode = useNodePlacement();
  const { plan, ruler: bandRuler, setRuler } = useBandPlan();
  const bookmarks = useQuery(bookmarksQuery());

  const plotRef = useRef<HTMLDivElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const traceRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallView | null>(null);
  const [seedFrame] = useState<SpectrumFrame | null>(() => spectrumHub.latest(set.id, stream));
  const frameRef = useRef<SpectrumFrame | null>(seedFrame);
  const gestureRef = useRef<Gesture | null>(null);
  const liveDbRef = useRef<Float32Array | null>(null);
  const tracesRef = useRef<TraceState | null>(null);
  const densityRef = useRef<DensityLayer | null>(null);
  const rowRef = useRef<Uint8Array | null>(null);
  const frozenDbRef = useRef<Float32Array | null>(null);

  const [meta, setMeta] = useState<FrameMeta | null>(() =>
    seedFrame === null ? null : metaOf(seedFrame),
  );
  const [glError, setGlError] = useState<string | null>(null);
  const [view, setView] = useState<SpectrumView>(FULL_VIEW);
  const [traceModes, setTraceModes] = useState<readonly TraceMode[]>([]);
  const [phosphor, setPhosphor] = useState(false);
  const [lock, setLock] = useState<DbWindow | null>(null);
  const [frozen, setFrozen] = useState<SpectrumHistory | null>(null);
  const [scrub, setScrub] = useState(0);
  const [waterfall, setWaterfall] = useState({ top: 0, height: 0, width: 0 });
  const [colormap, setColormap] = useState<Colormap>(readColormap);
  const [traceFraction, setTraceFraction] = useState(0.32);
  const [preview, setPreview] = useState<{
    channel: number;
    offsetHz: number;
  } | null>(null);
  const [panning, setPanning] = useState(false);
  const [picked, setPicked] = useState<number | null>(null);
  const [menu, setMenu] = useState<{
    pick: ScopePick;
    at: ScopeMenuAt;
    frame: string;
  } | null>(null);
  const [picker, setPicker] = useState<{ pick: ScopePick; frame: string } | null>(null);

  const viewRef = useRef(view);
  const modesRef = useRef(traceModes);
  const lockRef = useRef(lock);
  const frozenRef = useRef(frozen);
  const scrubRef = useRef(scrub);
  useLayoutEffect(() => {
    viewRef.current = view;
    modesRef.current = traceModes;
    lockRef.current = lock;
    frozenRef.current = frozen;
    scrubRef.current = scrub;
  });

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

  const tuneCenter = (hz: number): void =>
    applyPatch(set.id, tuneDelta(set.capabilities, stream, hz));
  const tuneChannel = (channel: number, offsetHz: number): void =>
    applyEdit(set.id, channel, { offset_hz: offsetHz });

  const tuneToBand = (hz: number, suggested: ChannelParams | null): void => {
    const params = suggested === null ? {} : { params: suggested };
    if (selectedChannel === null) {
      tuneCenter(hz);
      return;
    }
    if (meta === null || Math.abs(hz - meta.centerHz) >= meta.spanHz / 2) {
      tuneCenter(hz);
      applyEdit(set.id, selectedChannel, { offset_hz: 0, ...params });
    } else {
      applyEdit(set.id, selectedChannel, {
        offset_hz: Math.round(hz - meta.centerHz),
        ...params,
      });
    }
    const face = faces.get(selectedChannel);
    if (suggested === null || face === undefined) {
      return;
    }
    workspace.edit((current) => ({
      ...current,
      graph: patchNode(current.graph, face, (drawn) =>
        drawn.kind === "channel"
          ? {
              ...drawn,
              kind: "channel" as const,
              data: { channel_type: suggested.type },
            }
          : drawn,
      ),
    }));
  };

  const tuneTo = (pick: ScopePick): void => {
    if (selectedChannel !== null) {
      tuneChannel(selectedChannel, pick.offsetHz);
    } else {
      tuneCenter(pick.hz);
    }
  };

  const addChannelAt = (pick: ScopePick, channelType: string): void => {
    if (deviceNode === undefined) {
      return;
    }
    const id = newNodeId("channel");
    tuneOnCreate(id, pick.offsetHz);
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: addEdge(
        addNode(snapshot.graph, {
          id,
          kind: "channel",
          data: { channel_type: channelType },
          position: placeNode(snapshot.graph, "channel"),
        }),
        {
          from: { node: deviceNode, port: streamPort("iq", stream) },
          to: { node: id, port: "iq" },
        },
      ),
    }));
    workspace.apply();
    workspace.select(id);
  };

  const editRef = useRef(applyEdit);
  useLayoutEffect(() => {
    editRef.current = applyEdit;
  });

  useEffect(() => {
    for (const [channel, face] of faces) {
      const offsetHz = takeCreationTune(face);
      if (offsetHz !== undefined) {
        editRef.current(set.id, channel, { offset_hz: offsetHz });
      }
    }
  });

  useEffect(() => {
    const canvas = waterfallRef.current;
    if (canvas === null) {
      return;
    }
    let renderer: WaterfallView;
    try {
      renderer = attachWaterfall(canvas, setGlError);
    } catch (error) {
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

  useEffect(() => {
    const past = spectrumHub.history(set.id, stream);
    const opening = lockRef.current;
    rendererRef.current?.seed(
      opening === null ? past.rows : requantizeHistory(past, opening),
      past.count,
      past.bins,
    );
    let count = 0;
    return spectrumHub.subscribe(set.id, stream, (frame) => {
      frameRef.current = frame;
      const held = lockRef.current;
      const window = held ?? frameWindow(frame);
      const db = dequantize(frame, liveDbRef.current);
      liveDbRef.current = db;
      if (frozenRef.current === null) {
        if (held === null) {
          rendererRef.current?.pushRow(frame.bins);
        } else {
          const row = requantize(frame.bins, frameWindow(frame), held, rowRef.current);
          rowRef.current = row;
          rendererRef.current?.pushRow(row);
        }
        tracesRef.current = accumulateTraces(tracesRef.current, db);
        densityRef.current?.add(db, viewRef.current, window);
      }
      count += 1;
      if (count === 1 || count % 8 === 0) {
        setMeta(metaOf(frame));
      }
    });
  }, [set.id, stream]);

  useEffect(() => {
    let raf = 0;
    const loop = () => {
      const { frame, window } = plotSource(
        frozenRef.current,
        scrubRef.current,
        frameRef.current,
        liveDbRef.current,
        lockRef.current,
        frozenDbRef,
      );
      drawPlot(traceRef.current, {
        frame,
        view: viewRef.current,
        window,
        traces: overlays(tracesRef.current, modesRef.current, frame),
        density: densityRef.current,
      });
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);

  useEffect(() => {
    if (!phosphor) {
      densityRef.current = null;
      return;
    }
    const layer = new DensityLayer(colormap);
    densityRef.current = layer;
    return () => {
      if (densityRef.current === layer) {
        densityRef.current = null;
      }
    };
  }, [phosphor, colormap]);

  useEffect(() => {
    const canvas = waterfallRef.current;
    if (canvas === null) {
      return;
    }
    const measure = () =>
      setWaterfall({
        top: canvas.offsetTop,
        height: canvas.clientHeight,
        width: canvas.clientWidth,
      });
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    measure();
    return () => observer.disconnect();
  }, []);

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
    } catch {}
  };

  const reseed = (held: DbWindow | null): void => {
    const past = spectrumHub.history(set.id, stream);
    if (past.count === 0) {
      return;
    }
    rendererRef.current?.seed(
      held === null ? past.rows : requantizeHistory(past, held),
      past.count,
      past.bins,
    );
  };

  const toggleTrace = (mode: TraceMode): void => {
    setTraceModes((current) =>
      current.includes(mode) ? current.filter((name) => name !== mode) : [...current, mode],
    );
  };

  const toggleLock = (): void => {
    const next = lock === null ? (meta === null ? EMPTY_WINDOW : displayWindow(meta, null)) : null;
    lockRef.current = next;
    setLock(next);
    reseed(next);
    densityRef.current?.clear();
  };

  const toggleFreeze = (): void => {
    if (frozen !== null) {
      frozenRef.current = null;
      setFrozen(null);
      reseed(lockRef.current);
      return;
    }
    const captured = spectrumHub.history(set.id, stream);
    frozenRef.current = captured;
    setFrozen(captured);
    setScrub(Math.max(0, frozenLength(captured) - 1));
  };

  const frozenRows = frozen === null ? 0 : frozenLength(frozen);
  const cursorAt =
    frozen === null
      ? null
      : frozenCursor(scrub, frozenRows, rowsForHeight(waterfall.height, 1, SPECTRUM_HISTORY_ROWS));

  const spanHz = meta?.spanHz ?? 0;
  const pointerFraction = (clientX: number): number => {
    const rect = plotRef.current?.getBoundingClientRect();
    return rect === undefined || rect.width === 0 ? 0 : (clientX - rect.left) / rect.width;
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (!active || event.button !== 0 || plotRef.current === null || spanHz <= 0) {
      return;
    }
    if (!onPlotSurface(event.target, plotRef.current)) {
      return;
    }
    const rect = plotRef.current.getBoundingClientRect();
    const at = pointerFraction(event.clientX);
    const grabbed = markerAt(set.channels, view, spanHz, at, GRAB_PX / rect.width);
    if (grabbed !== null) {
      selectChannel(grabbed.id);
    }
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
      setPreview({
        channel: gesture.channel,
        offsetHz: Math.round(spanToOffset(viewToSpan(gesture.view, at), spanHz)),
      });
      return;
    }
    setPanning(true);
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
    if (meta === null || gesture.channel !== null) {
      return;
    }
    const offsetHz = Math.round(spanToOffset(viewToSpan(gesture.view, gesture.at), meta.spanHz));
    if (selectedChannel !== null) {
      tuneChannel(selectedChannel, offsetHz);
    } else {
      tuneCenter(meta.centerHz + offsetHz);
    }
  };

  const frameStamp = `${meta?.centerHz}:${meta?.spanHz}:${view.start}:${view.end}`;
  const openMenu = menu?.frame === frameStamp ? menu : null;
  const openPicker = picker?.frame === frameStamp ? picker : null;
  const suggestedType = (hz: number): string =>
    channelTypeAt(
      plan === null ? null : suggestedAt(identify(plan, hz)),
      set.channels.find((channel) => channel.id === selectedChannel),
    );

  const onContextMenu = (event: React.MouseEvent<HTMLDivElement>): void => {
    const plot = plotRef.current;
    if (!active || meta === null || plot === null || !onPlotSurface(event.target, plot)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const rect = plot.getBoundingClientRect();
    const at = pointerFraction(event.clientX);
    setMenu({
      pick: pickAt(meta.centerHz, meta.spanHz, view, at),
      at: {
        x: at,
        y: rect.height === 0 ? 0 : (event.clientY - rect.top) / rect.height,
      },
      frame: frameStamp,
    });
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
      onContextMenu={onContextMenu}
      onDoubleClick={(event) => {
        const plot = plotRef.current;
        if (!active || meta === null || plot === null || !onPlotSurface(event.target, plot)) {
          return;
        }
        const at = pointerFraction(event.clientX);
        tuneCenter(Math.round(meta.centerHz + spanToOffset(viewToSpan(view, at), meta.spanHz)));
        setView(FULL_VIEW);
      }}
    >
      {meta !== null && (
        <BandRuler centerHz={meta.centerHz} spanHz={meta.spanHz} view={view} onTune={tuneToBand} />
      )}
      <canvas
        ref={traceRef}
        className="w-full shrink-0"
        style={{ height: `${traceFraction * 100}%` }}
      />
      <Divider fraction={traceFraction} onFraction={setTraceFraction} plotRef={plotRef} />
      <canvas ref={waterfallRef} className="w-full min-h-0 flex-1" />

      {cursorAt !== null && (
        <div
          className="pointer-events-none absolute inset-x-0 border-t border-plot-ink/80"
          style={{ top: `${waterfall.top + cursorAt * waterfall.height}px` }}
        />
      )}

      {meta !== null && (
        <Bookmarks
          bookmarks={bookmarks.data ?? []}
          centerHz={meta.centerHz}
          spanHz={meta.spanHz}
          view={view}
          labelTop={waterfall.top + 3}
          widthPx={waterfall.width}
        />
      )}

      {meta !== null && (
        <Markers
          channels={set.channels}
          view={view}
          spanHz={meta.spanHz}
          selected={selectedChannel}
          preview={preview}
          onSelect={selectChannel}
          widthPx={waterfall.width}
        />
      )}

      <div className="pointer-events-none absolute inset-0 flex flex-col justify-between p-1.5">
        <span className="legend self-end text-right whitespace-pre text-plot-ink-dim">
          {meta !== null && `${formatCentre(meta, view)}${formatRange(displayWindow(meta, lock))}`}
          {lock !== null && " · held"}
        </span>
        <div
          data-plot-chrome
          className="pointer-events-auto flex items-center gap-1 self-start rounded-[3px] bg-plot-bg/85 p-0.5"
        >
          <Popover
            label={colormap}
            triggerClass={plotButton(false)}
            width="w-auto min-w-[var(--anchor-width)]"
            padded={false}
          >
            {(close) => (
              <div className="flex flex-col p-0.5">
                {COLORMAPS.map((name) => (
                  <Button
                    key={name}
                    type="button"
                    className={`${segment(name === colormap)} justify-start`}
                    onClick={() => {
                      chooseColormap(name);
                      close();
                    }}
                  >
                    {name}
                  </Button>
                ))}
              </div>
            )}
          </Popover>
          <Popover
            label={traceLabel(traceModes, phosphor)}
            triggerClass={plotButton(traceModes.length > 0 || phosphor)}
            width="w-auto min-w-[var(--anchor-width)]"
            padded={false}
          >
            {() => (
              <div className="flex flex-col p-0.5">
                {TRACE_MODES.map((mode) => (
                  <Button
                    key={mode}
                    type="button"
                    className={`${segment(traceModes.includes(mode))} justify-start`}
                    aria-pressed={traceModes.includes(mode)}
                    onClick={() => toggleTrace(mode)}
                  >
                    {mode}
                  </Button>
                ))}
                <Button
                  type="button"
                  className={`${segment(phosphor)} justify-start`}
                  aria-pressed={phosphor}
                  onClick={() => {
                    if (!phosphor && lock === null) {
                      toggleLock();
                    }
                    setPhosphor(!phosphor);
                  }}
                >
                  phosphor
                </Button>
                <Button
                  type="button"
                  className={`${segment(lock !== null)} justify-start`}
                  aria-pressed={lock !== null}
                  onClick={toggleLock}
                >
                  hold dB range
                </Button>
              </div>
            )}
          </Popover>
          <Button
            type="button"
            className={plotButton(bandRuler)}
            aria-pressed={bandRuler}
            onClick={() => setRuler(!bandRuler)}
          >
            bands
          </Button>
          <Button
            type="button"
            className={plotButton(frozen !== null)}
            aria-pressed={frozen !== null}
            onClick={toggleFreeze}
          >
            {frozen === null ? "freeze" : "live"}
          </Button>
          {!isFullView(view) && (
            <Button type="button" className={plotButton(false)} onClick={() => setView(FULL_VIEW)}>
              {(1 / viewWidth(view)).toFixed(1)}× · reset
            </Button>
          )}
        </div>
      </div>

      {frozen !== null && frozenRows > 0 && (
        <div
          data-plot-chrome
          className="absolute inset-x-1.5 bottom-8 flex items-center gap-2 rounded-[3px] bg-plot-bg/85 px-1.5 py-1"
        >
          <Slider
            label="Scrub the frozen waterfall"
            className="min-w-0 flex-1"
            min={0}
            max={frozenRows - 1}
            value={Math.min(scrub, frozenRows - 1)}
            onChange={setScrub}
          />
          <span className="legend w-16 shrink-0 text-right whitespace-pre text-plot-ink-dim">
            {frozenAge(frozen, scrub)}
          </span>
        </div>
      )}

      {openMenu !== null && (
        <ScopeMenu
          pick={openMenu.pick}
          at={openMenu.at}
          draft={bookmarkDraft(openMenu.pick.hz, plan)}
          onTune={() => {
            tuneTo(openMenu.pick);
            setMenu(null);
          }}
          onChannel={() => {
            setPicker({ pick: openMenu.pick, frame: frameStamp });
            setMenu(null);
          }}
          onClose={() => setMenu(null)}
        />
      )}

      {openPicker !== null && (
        <ChannelPicker
          pick={openPicker.pick}
          channelTypes={workspace.context.channelTypes}
          suggested={suggestedType(openPicker.pick.hz)}
          onChannel={(channelType) => {
            addChannelAt(openPicker.pick, channelType);
            setPicker(null);
          }}
          onClose={() => setPicker(null)}
        />
      )}

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
      data-plot-chrome
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
        className="absolute inset-x-0 top-1 h-px bg-plot-ink-dim/45 group-hover:bg-accent"
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
  widthPx,
}: {
  channels: readonly ChannelInfo[];
  view: SpectrumView;
  spanHz: number;
  selected: number | null;
  preview: { channel: number; offsetHz: number } | null;
  onSelect: (channel: number) => void;
  widthPx: number;
}) {
  const visible = spanHz * viewWidth(view);
  const drawn = channels
    .map((channel) => {
      const offsetHz =
        preview?.channel === channel.id ? preview.offsetHz : (channel.settings.offset_hz ?? 0);
      return {
        channel,
        offsetHz,
        id: channel.id,
        at: spanToView(view, offsetToSpan(offsetHz, spanHz)),
        width: labelWidth(markerName(channel, offsetHz), widthPx),
      };
    })
    .filter((marker) => marker.at >= -0.02 && marker.at <= 1.02);
  return (
    <div className="pointer-events-none absolute inset-0 overflow-hidden">
      {clusterMarkers(drawn).map((members) => {
        const anchor = members[0];
        if (anchor === undefined) {
          return null;
        }
        const shown = members.find((member) => member.channel.id === selected) ?? anchor;
        const stacked = members.length > 1;
        return (
          <div key={anchor.channel.id}>
            {members.map(({ channel, offsetHz, at }) => {
              const active = channel.id === selected;
              const bandwidth = bandwidthHz(channel.settings.params);
              return (
                <Fragment key={channel.id}>
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
                  <Button
                    type="button"
                    className="pointer-events-auto absolute inset-y-0 w-5 -translate-x-1/2 cursor-ew-resize"
                    style={{ left: `${at * 100}%` }}
                    onClick={(event) => {
                      event.stopPropagation();
                      onSelect(channel.id);
                    }}
                    aria-label={`${channel.settings.params.type} channel at ${formatSignedKhz(offsetHz)} — drag to tune`}
                  />
                </Fragment>
              );
            })}

            <div
              className="pointer-events-auto group absolute flex -translate-x-1/2 flex-col items-center gap-1"
              style={{ left: `${shown.at * 100}%`, top: LABEL_TOP_PX }}
            >
              <MarkerLabel
                active={shown.channel.id === selected}
                className={stacked ? "group-hover:hidden" : ""}
              >
                {markerName(shown.channel, shown.offsetHz)}
                {stacked && <span className="ml-1 text-plot-ink-dim">×{members.length}</span>}
              </MarkerLabel>
              {stacked &&
                members.map(({ channel, offsetHz }) => (
                  <MarkerLabel
                    key={channel.id}
                    active={channel.id === selected}
                    className="hidden group-hover:block"
                  >
                    {markerName(channel, offsetHz)}
                  </MarkerLabel>
                ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function Bookmarks({
  bookmarks,
  centerHz,
  spanHz,
  view,
  labelTop,
  widthPx,
}: {
  bookmarks: readonly Bookmark[];
  centerHz: number;
  spanHz: number;
  view: SpectrumView;
  labelTop: number;
  widthPx: number;
}) {
  if (!(spanHz > 0)) {
    return null;
  }
  const drawn = bookmarks
    .map((bookmark) => ({
      bookmark,
      at: spanToView(view, offsetToSpan(bookmark.freq_hz - centerHz, spanHz)),
      width: labelWidth(bookmark.label, widthPx),
    }))
    .filter((mark) => mark.at >= 0 && mark.at <= 1);
  return (
    <div className="pointer-events-none absolute inset-0 overflow-hidden">
      {drawn.map(({ bookmark, at }) => (
        <span
          key={bookmark.id}
          aria-hidden
          className="absolute inset-y-0 w-0 border-l border-dashed border-accent/50"
          style={{ left: `${at * 100}%` }}
        />
      ))}
      {clusterMarkers(drawn).map((members) => {
        const anchor = members[0];
        if (anchor === undefined) {
          return null;
        }
        return (
          <span
            key={anchor.bookmark.id}
            title={members
              .map((mark) => `${mark.bookmark.label} — ${formatMhz(mark.bookmark.freq_hz)}`)
              .join("\n")}
            className="absolute -translate-x-1/2 rounded-[2px] border border-accent/40 bg-bg/85 px-1 py-px font-mono text-[10px] whitespace-nowrap text-accent"
            style={{ left: `${anchor.at * 100}%`, top: `${labelTop}px` }}
          >
            {anchor.bookmark.label}
            {members.length > 1 && <span className="ml-1 text-ink-dim">×{members.length}</span>}
          </span>
        );
      })}
    </div>
  );
}

function markerName(channel: ChannelInfo, offsetHz: number): string {
  return `${channel.settings.params.type.toUpperCase()} ${formatSignedKhz(offsetHz)}`;
}

function MarkerLabel({
  active,
  className,
  children,
}: {
  active: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      aria-hidden
      className={`rounded-[2px] border px-1 py-px font-mono text-[10px] whitespace-nowrap tabular-nums ${
        active ? "border-accent bg-bg text-accent" : "border-line bg-bg/85 text-ink-dim"
      } ${className ?? ""}`}
    >
      {children}
    </span>
  );
}

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

function onPlotSurface(target: EventTarget | null, plot: HTMLElement): boolean {
  if (!(target instanceof Node) || !plot.contains(target)) {
    return false;
  }
  return !(target instanceof Element) || target.closest("[data-plot-chrome]") === null;
}

function metaOf(frame: SpectrumFrame): FrameMeta {
  return {
    centerHz: frame.centerHz,
    spanHz: frame.spanHz,
    dbMin: frame.dbMin,
    dbMax: frame.dbMax,
  };
}

function readColormap(): Colormap {
  try {
    const stored = localStorage.getItem(COLORMAP_KEY);
    return COLORMAPS.find((name) => name === stored) ?? DEFAULT_COLORMAP;
  } catch {
    return DEFAULT_COLORMAP;
  }
}

function formatCentre(meta: FrameMeta, view: SpectrumView): string {
  const visible = meta.spanHz * viewWidth(view);
  const centre = meta.centerHz + spanToOffset((view.start + view.end) / 2, meta.spanHz);
  const span =
    visible >= 1e6 ? `${(visible / 1e6).toFixed(3)} MHz` : `${(visible / 1e3).toFixed(1)} kHz`;
  return `${(centre / 1e6).toFixed(4)} MHz   ${span}`;
}

function formatRange(window: DbWindow): string {
  return `   ${window.min.toFixed(0)}…${window.max.toFixed(0)} dB`;
}

function traceLabel(modes: readonly TraceMode[], phosphor: boolean): string {
  const on = [...modes, ...(phosphor ? (["phosphor"] as const) : [])];
  return on.length === 0 ? "traces" : on.join(" · ");
}

function displayWindow(meta: FrameMeta | null, held: DbWindow | null): DbWindow {
  if (held !== null) {
    return held;
  }
  return meta === null ? EMPTY_WINDOW : { min: meta.dbMin, max: meta.dbMax };
}

function plotSource(
  frozen: SpectrumHistory | null,
  scrub: number,
  frame: SpectrumFrame | null,
  liveDb: Float32Array | null,
  held: DbWindow | null,
  scratch: RefObject<Float32Array | null>,
): { frame: PlotFrame | null; window: DbWindow } {
  if (frozen !== null) {
    const row = frozenRow(frozen, scrub, scratch.current);
    scratch.current = row?.db ?? null;
    return {
      frame: row === null ? null : { centerHz: row.centerHz, spanHz: row.spanHz, db: row.db },
      window: held ?? row?.window ?? EMPTY_WINDOW,
    };
  }
  if (frame === null || liveDb === null) {
    return { frame: null, window: held ?? EMPTY_WINDOW };
  }
  return {
    frame: { centerHz: frame.centerHz, spanHz: frame.spanHz, db: liveDb },
    window: held ?? frameWindow(frame),
  };
}

function overlays(
  state: TraceState | null,
  modes: readonly TraceMode[],
  frame: PlotFrame | null,
): PlotTrace[] {
  if (state === null || frame === null) {
    return [];
  }
  const traces: PlotTrace[] = [];
  for (const mode of modes) {
    const db = traceOf(state, mode);
    if (db.length === frame.db.length) {
      traces.push({ mode, db });
    }
  }
  return traces;
}
