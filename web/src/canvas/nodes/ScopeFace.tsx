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
import { basebandSourceOf, channelNodesOf, hasBasebandWire, iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { addEdge, addNode, newNodeId, patchNode, streamPort } from "../graph";
import { useNodePlacement } from "../placement";
import { deviceSetOf } from "../workspaceDevice";
import { BandRuler } from "./BandRuler";
import { BasebandView } from "./BasebandView";
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
/** How close the pointer must be to a marker to grab it rather than pan the plot. */
const GRAB_PX = 10;
const COLORMAP_KEY = "sdrmm.colormap";
const TRACE_MIN = 0.15;
const TRACE_MAX = 0.75;
/** Where a marker's label sits, under the band ruler. */
const LABEL_TOP_PX = 28;
/** What the plot maps onto its height before any frame has arrived to say otherwise. */
const EMPTY_WINDOW: DbWindow = { min: -100, max: -20 };

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

/** The two instruments a scope can be, in the order the wires are drawn into it. */
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
  const [chosen, setChosen] = useState<ScopeSource>("baseband");
  const shown = scopeSource(chosen, source !== null, tap !== null);

  // Only a scope holding both wires has anything to switch between; with one, the toggle would be
  // a control with a single position. It sits in the title bar rather than in the plot's own
  // toolbar because it decides *which* toolbar is drawn.
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
          // The IQ wire is named first: this branch is the spectrum, so a scope holding both
          // wires is here because its radio is the missing half, not its channel.
          <FaceEmpty>
            {source !== null
              ? "The radio this scope watches is not attached. The wire is kept."
              : hasBasebandWire(workspace.graph, node.id)
                ? "The channel this scope taps is not running. The wire is kept."
                : "Wire a device's IQ out to watch its spectrum, or a channel's baseband out to watch one channel."}
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
  const placeNode = useNodePlacement();
  // Read here and not only inside the ruler: the toolbar toggle has to reflect it too, and the
  // setting is the workspace's, so every scope on the canvas draws the same answer.
  const { plan, ruler: bandRuler, setRuler } = useBandPlan();
  const bookmarks = useQuery(bookmarksQuery());

  const plotRef = useRef<HTMLDivElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const traceRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<WaterfallView | null>(null);
  // The hub kept the last frame that arrived while this face did not exist (lib/spectrum.ts).
  // Read at first render rather than in an effect: the trace and the readout have to be there in
  // the rack's first paint, or the switch still shows the blank the history exists to remove.
  // `Spectrum` is keyed by lane, so a different lane is a different mount reading its own.
  const [seedFrame] = useState<SpectrumFrame | null>(() => spectrumHub.latest(set.id, stream));
  const frameRef = useRef<SpectrumFrame | null>(seedFrame);
  const gestureRef = useRef<Gesture | null>(null);
  /** The live frame expanded to dBFS. Every trace, the phosphor and the plot read this rather
   * than the bytes: the window they are drawn against is not always the frame's own. */
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
  /** The dB range the plot is pinned to, or `null` to follow whatever the server sends. */
  const [lock, setLock] = useState<DbWindow | null>(null);
  /** The history a freeze captured, and how far back into it the operator has scrubbed. */
  const [frozen, setFrozen] = useState<SpectrumHistory | null>(null);
  const [scrub, setScrub] = useState(0);
  /** Where the waterfall sits inside the plot, in pixels. Measured rather than derived from the
   * trace's fraction: the band ruler is a flex sibling above the trace, so the split is that
   * fraction of the *remaining* height and every overlay placed on the waterfall from the
   * fraction alone lands one ruler too high — on the frequency axis. The canvas runs the full
   * width of the plot, so its width is also every overlay's. */
  const [waterfall, setWaterfall] = useState({ top: 0, height: 0, width: 0 });
  const [colormap, setColormap] = useState<Colormap>(readColormap);
  const [traceFraction, setTraceFraction] = useState(0.32);
  const [preview, setPreview] = useState<{
    channel: number;
    offsetHz: number;
  } | null>(null);
  const [panning, setPanning] = useState(false);
  const [picked, setPicked] = useState<number | null>(null);
  /** The right-click menu, stamped with the frame it was opened in — a pan, a zoom or a retune
   * moves the spectrum out from under it, and a menu still naming the old frequency is worse
   * than no menu. */
  const [menu, setMenu] = useState<{
    pick: ScopePick;
    at: ScopeMenuAt;
    frame: string;
  } | null>(null);

  // The animation-frame loop and the frame subscription both outlive the render that set these,
  // so they read the view and the display switches here. Written after commit, never during
  // render: React may replay or discard a render, and the loop must not see a value from one
  // that never landed.
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

  /** Tune whatever the operator is working on to a picked frequency: the selected channel where
   * there is one, and the receiver itself where there is not. The same rule a click follows. */
  const tuneTo = (pick: ScopePick): void => {
    if (selectedChannel !== null) {
      tuneChannel(selectedChannel, pick.offsetHz);
    } else {
      tuneCenter(pick.hz);
    }
  };

  /** Draw a channel at a picked frequency and wire it to the lane this scope is watching.
   *
   * The node carries the type; the frequency is a setting on the engine channel apply has yet to
   * create, so it is left with `tuneOnCreate` for the effect below to land. */
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

  // `applyEdit` is rebuilt every render, and the tune below must not be keyed on that.
  const editRef = useRef(applyEdit);
  useLayoutEffect(() => {
    editRef.current = applyEdit;
  });

  // The opening tune of a channel drawn on the plot, landed on the first render that finds the
  // engine channel behind its node. Unconditional rather than keyed: `takeCreationTune` clears as
  // it reads, so every render after the first is a handful of misses.
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
      // A frozen plot still records — the hub is what it will be scrubbed over — but nothing that
      // moves under the operator's eye is advanced while they are reading it.
      if (frozenRef.current === null) {
        if (held === null) {
          rendererRef.current?.pushRow(frame.bins);
        } else {
          // The texture is one byte per bin with no room for a per-row window, so a plot pinned to
          // a fixed range has to convert on the way in.
          const row = requantize(frame.bins, frameWindow(frame), held, rowRef.current);
          rowRef.current = row;
          rendererRef.current?.pushRow(row);
        }
        tracesRef.current = accumulateTraces(tracesRef.current, db);
        densityRef.current?.add(db, viewRef.current, window);
      }
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

  // The phosphor layer exists only while it is switched on: it owns a bitmap and a grid, and a
  // scope that is not showing one should not be paying for it.
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

  // How far back the waterfall reaches — which is what turns a scrub index into a cursor position
  // — and where it begins, which is where anything drawn over it belongs.
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

  /** Re-upload every row the hub has kept under `held`. The rows already in the texture were
   * quantized against whatever window was current when they arrived, so changing the plot's own
   * window has to re-colour the history as well as the frames still to come. */
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

  /** Pin the plot to the window it is showing right now, or let it follow the server again.
   *
   * Pinning is what makes the accumulated traces and the phosphor comparable frame to frame: the
   * server widens its window whenever a burst arrives, and a display that follows it re-scales
   * everything already drawn underneath. */
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
      // The lane kept filling while the plot was held; catching the texture up is what stops the
      // waterfall from resuming with a seam in it.
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
    // An inactive face is brought forward by the click and nothing else: the pointer belongs to
    // the camera there, and a plot that tuned on first contact would retune a radio the operator
    // was only reaching past.
    if (!active || event.button !== 0 || plotRef.current === null || spanHz <= 0) {
      return;
    }
    // The plot declines what is not its own surface, rather than every control stopping
    // propagation: the popover is Base UI's, and its dismissal listens where a swallowed event
    // would never arrive.
    if (!onPlotSurface(event.target, plotRef.current)) {
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

  // A menu opened against one window must not survive a pan, a zoom or someone else's retune —
  // the same rule the band ruler's identify card follows, and stamped rather than cleared from an
  // effect so the stale card never reaches the paint after the gesture.
  const frameStamp = `${meta?.centerHz}:${meta?.spanHz}:${view.start}:${view.end}`;
  const openMenu = menu?.frame === frameStamp ? menu : null;
  const menuType =
    openMenu === null
      ? null
      : channelTypeAt(
          plan === null ? null : suggestedAt(identify(plan, openMenu.pick.hz)),
          set.channels.find((channel) => channel.id === selectedChannel),
        );

  const onContextMenu = (event: React.MouseEvent<HTMLDivElement>): void => {
    const plot = plotRef.current;
    // An inactive face leaves the pointer to the canvas, whose own menu offers what can be done
    // to the *node*. Bringing it forward first is what makes the frequency under the pointer a
    // thing this scope can speak about at all.
    if (!active || meta === null || plot === null || !onPlotSurface(event.target, plot)) {
      return;
    }
    event.preventDefault();
    // React Flow opens the node menu from a `contextmenu` on the node wrapper this plot sits in;
    // two menus over one click is one too many.
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
        // Guarded here too, and not only in `onPointerDown`: this path never consults the
        // gesture, so two quick jabs at a toolbar button would recentre the radio.
        const plot = plotRef.current;
        if (!active || meta === null || plot === null || !onPlotSurface(event.target, plot)) {
          return;
        }
        const at = pointerFraction(event.clientX);
        tuneCenter(Math.round(meta.centerHz + spanToOffset(viewToSpan(view, at), meta.spanHz)));
        setView(FULL_VIEW);
      }}
    >
      {/* Above the trace and outside the plot rectangle, sharing its width so the two axes are
          the same axis (). */}
      {meta !== null && (
        <BandRuler centerHz={meta.centerHz} spanHz={meta.spanHz} view={view} onTune={tuneToBand} />
      )}
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

      {/* The row the trace above is showing. Achromatic, like every other overlay on the
          waterfall: the colormap owns hue inside that rectangle. */}
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
        {/* Bottom-left: the only corner of the plot no data occupies, so the toolbar costs the
            trace nothing. It carries its own scrim of the plot ground — the waterfall reaches
            this corner and a colormap's low end is not always dark (Classic's is a saturated
            blue), so the labels cannot borrow their contrast from whatever is underneath. */}
        <div
          data-plot-chrome
          className="pointer-events-auto flex items-center gap-1 self-start rounded-[3px] bg-plot-bg/85 p-0.5"
        >
          {/* Sized like `Select`'s list and not like a panel: six one-word choices, so the
              popup shrinks to the longest of them and only holds the trigger's width as a
              floor. A fixed width here left most of every row empty. */}
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
          {/* Everything that changes what the plot draws from the same stream, behind one
              trigger: the toolbar sits over the trace, and six switches along it would cost more
              of the plot than they are worth. */}
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
                    // A phosphor built over a window that keeps moving smears every level it has
                    // drawn, so switching it on pins the display to what is on screen now.
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

      {/* The scrub bar rides above the toolbar rather than inside it: it is as wide as the plot,
          and it only exists while the plot is frozen. */}
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

      {openMenu !== null && menuType !== null && (
        <ScopeMenu
          pick={openMenu.pick}
          at={openMenu.at}
          channelType={menuType}
          draft={bookmarkDraft(openMenu.pick.hz, plan)}
          onTune={() => {
            tuneTo(openMenu.pick);
            setMenu(null);
          }}
          onChannel={() => {
            addChannelAt(openMenu.pick, menuType);
            setMenu(null);
          }}
          onClose={() => setMenu(null)}
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
  /** How wide the plot is drawn, in pixels: what decides whether two captions collide. */
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
    // Markers must not steal gestures from the plot: the layer is pointer-transparent and only
    // each marker's own hit strip takes the pointer.
    <div className="pointer-events-none absolute inset-0 overflow-hidden">
      {clusterMarkers(drawn).map((members) => {
        const anchor = members[0];
        if (anchor === undefined) {
          return null;
        }
        // Which one the collapsed label speaks for: whichever the operator is working on, so
        // picking a channel names it even where five others share its frequency.
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
                    // The hit strip is invisible and wide; the drawn line stays 1px, because ink
                    // and target size are different budgets.
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

            {/* The label is its own hover target, and only as big as it looks: the hit strip
                beside it runs the height of the plot, and opening the stack from there meant
                brushing anywhere near the trace unfolded it. Taking the pointer costs the plot
                nothing — a press here still bubbles to the plot's own handlers, which hit-test
                by x and grab the same marker. */}
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
              {/* Unfolded only while the pointer is on the label: a permanent stack costs the
                  trace a label's height per channel, which is the whole plot on a busy one. */}
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

/**
 * Saved frequencies that fall inside the window, as dashed ticks along the plot.
 *
 * Coloured where the channel markers are achromatic, and labelled at the top of the waterfall
 * rather than under the ruler: a bookmark is something the operator wrote down, not something the
 * receiver is doing, and the two must never be read as the same kind of mark. Inert — a bookmark
 * is tuned from the menu that made it, and a hit strip here would take the pointer away from the
 * channel markers that are dragged.
 *
 * The tick is decorative and the caption is not: with colour and position removed, the name and
 * the frequency it carries are the whole of what a mark says.
 */
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
  /** Where the labels sit, in pixels down the plot: just inside the waterfall. */
  labelTop: number;
  /** How wide the plot is drawn, in pixels: what decides whether two captions collide. */
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

/** One marker's caption, in the flow of its cluster's label column. */
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

/**
 * Whether a pointer that reached the plot's handlers actually landed on the plot. Two ways it did
 * not, and the plot has to decline both: it captures the pointer to pan and tune, and a capture
 * on the ancestor retargets the release, so a control underneath never sees a click and the
 * tune-on-click runs in its place.
 *
 * Chrome drawn over the plot — the toolbar, the split handle — is marked, because it is inside
 * the plot and hit-testing alone cannot tell it apart. A portalled popup is the other way round:
 * React dispatches synthetic events up the *component* tree, so a popover this subtree renders
 * into `document.body` bubbles in here from outside the plot entirely.
 *
 * Channel markers are deliberately neither: dragging one to tune is a plot gesture that begins on
 * a marker, and the plot has to receive it.
 */
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

/** What the display popover's trigger says: the switches that are on, or the word for none. */
function traceLabel(modes: readonly TraceMode[], phosphor: boolean): string {
  const on = [...modes, ...(phosphor ? (["phosphor"] as const) : [])];
  return on.length === 0 ? "traces" : on.join(" · ");
}

/** The dB range the plot maps onto its height: the pinned one if there is one, else whatever the
 * server measured this frame under. */
function displayWindow(meta: FrameMeta | null, held: DbWindow | null): DbWindow {
  if (held !== null) {
    return held;
  }
  return meta === null ? EMPTY_WINDOW : { min: meta.dbMin, max: meta.dbMax };
}

/** What the plot is drawing right now — the scrubbed row of a frozen history, or the live frame.
 * `scratch` is the frozen row's buffer, reused so scrubbing does not allocate per animation
 * frame. */
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

/** The accumulated traces to draw over the live one. A trace whose length no longer matches the
 * frame is dropped rather than stretched: a changed bin count is a different frequency axis, and
 * the accumulator resets on the next frame anyway. */
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
