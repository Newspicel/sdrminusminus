import { Button } from "./BaseControls";
import "maplibre-gl/dist/maplibre-gl.css";
import {
  AttributionControl,
  type GeoJSONSource,
  Map as MapLibreMap,
  type MapMouseEvent,
  type MapOptions,
  NavigationControl,
  setWorkerUrl,
} from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { useEffect, useRef, useState } from "react";
import { useDecodedStore } from "../lib/decoded";
import { trailBounds, unwrapTrail } from "../lib/map/bounds";
import {
  AGE_OUT_INTERVAL_MS,
  DRAW_TICK_MS,
  KIND_STYLE,
  layerId,
  MAP_KINDS,
  type MapKind,
  mapKindsOf,
  referenceCollection,
  sourceId,
  TARGET_MAX_AGE_MS,
  type Target,
  type TargetCollection,
  type TargetDetail,
  targetCollection,
  targetDetail,
} from "../lib/map/layers";
import { type PositionSample, usePositionStore } from "../lib/position";
import { SIGNAL_MAX_DBFS, SIGNAL_MIN_DBFS, type SignalSurveySample } from "../lib/signalSurvey";
import { formatMhz } from "./format";

// MapLibre v6 ships its worker as a separate file and derives its URL from `import.meta.url`,
// which under a bundler points at the bundle rather than the package — so every bundler consumer
// has to hand it the worker itself. `?worker&url` and not `?url`: the dist worker imports a
// sibling chunk that a verbatim asset copy would leave behind.
setWorkerUrl(workerUrl);

/** : OpenFreeMap vector tiles — free, no API key, no usage cap. A self-hosted PMTiles
 * basemap replaces this URL when the server grows one; nothing else here changes. */
const BASEMAP_STYLE_URL = "https://tiles.openfreemap.org/styles/liberty";

/** A field Pi has no internet and must not wait on one: if the style has not arrived by then,
 * the map opens on the offline backdrop instead of hanging with a blank canvas. */
const BASEMAP_TIMEOUT_MS = 6_000;

/** Half-size of the click/tap hit box in pixels — a 4 px dot is not a touch target. */
const HIT_SLOP_PX = 9;

/** MapLibre's compact attribution opens *expanded* and re-expands itself whenever the credits
 * change (`_updateAttributions` calls `_updateCompact`), so a map node spends its bottom edge on a
 * credit line the operator never asked for. `maplibregl-compact` present without
 * `maplibregl-compact-show` is the collapsed ⓘ badge — the same state MapLibre's own toggle lands
 * in — and holding the class from the start is what makes it stick: `_updateCompact` expands only
 * when that class is *absent*. */
class CollapsedAttributionControl extends AttributionControl {
  override onAdd(map: MapLibreMap): HTMLElement {
    const container = super.onAdd(map);
    container.classList.add("maplibregl-compact");
    container.classList.remove("maplibregl-compact-show");
    return container;
  }
}

type MapStyle = Exclude<NonNullable<MapOptions["style"]>, string>;
type Counts = Record<MapKind, number>;

const EMPTY_COLLECTION: TargetCollection = { type: "FeatureCollection", features: [] };
const ZERO_COUNTS: Counts = { adsb: 0, ais: 0, aprs: 0 };

export function MapPanel({
  kinds,
  references = [],
  positionNodes = [],
  signalSamples,
  active = true,
  className,
}: {
  kinds: readonly MapKind[];
  /** `[lon, lat]` station fixes — an ADS-B channel's CPR reference — drawn as landmarks under
   * the targets they anchor. */
  references?: readonly (readonly [number, number])[];
  positionNodes?: readonly string[];
  /** Aggregated drive-survey readings. Present, even when empty, enables the signal layer and
   * its absolute dBFS legend. */
  signalSamples?: readonly SignalSurveySample[];
  /** Whether the map owns the pointer and the wheel. On the canvas it does so only while its node
   * is the active face — MapLibre's own handlers would otherwise pan the map *and* the patch with
   * one gesture, since the two cannot share a wheel. */
  active?: boolean;
  className?: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const readyRef = useRef(false);
  // Last station array published per kind. Comparing identity (the store hands out a new array
  // only when that kind changed) keeps an idle map from re-serialising GeoJSON twice a second.
  const drawnRef = useRef<Partial<Record<MapKind, readonly Target[]>>>({});
  const selectedRef = useRef<{ kind: MapKind; id: string } | null>(null);
  const targetFramedRef = useRef(false);
  const positionFramedRef = useRef(false);
  const signalFramedRef = useRef(false);
  // The map is built once and outlives any number of wire changes, so the listeners and the draw
  // tick read the wired kinds, the references and the theme colours from here rather than
  // closing over them.
  const kindsRef = useRef(kinds);
  kindsRef.current = kinds;
  const referencesRef = useRef(references);
  referencesRef.current = references;
  const positionNodesRef = useRef(positionNodes);
  positionNodesRef.current = positionNodes;
  const signalSamplesRef = useRef<readonly SignalSurveySample[] | null>(signalSamples ?? null);
  signalSamplesRef.current = signalSamples ?? null;
  const positionDrawnRef = useRef("");
  const signalDrawnRef = useRef<readonly SignalSurveySample[] | null>(null);
  const edgeRef = useRef("");
  const accentRef = useRef("");

  const countsRef = useRef<Counts>(ZERO_COUNTS);

  const [counts, setCounts] = useState<Counts>(ZERO_COUNTS);
  const [detail, setDetail] = useState<TargetDetail | null>(null);
  const [basemap, setBasemap] = useState<"pending" | "online" | "offline">("pending");
  const [positionCount, setPositionCount] = useState(0);
  const [signalCount, setSignalCount] = useState(0);

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    let disposed = false;

    const select = (next: { kind: MapKind; id: string } | null): void => {
      selectedRef.current = next;
      setDetail(next === null ? null : findDetail(next.kind, next.id));
      highlight(mapRef.current, kindsRef.current, next);
    };

    // One token does double duty: the offline backdrop, and the outline that keeps every target
    // readable against whichever basemap is under it.
    const edge = themeColor(container, "--color-bg", "#101113");
    edgeRef.current = edge;
    // The station mark is *ours* — the one place the map spends the app accent rather than a
    // kind colour, so it can never be mistaken for a target.
    accentRef.current = themeColor(container, "--color-accent", "#76acfc");

    void (async () => {
      const style = await fetchStyle();
      if (disposed) {
        return;
      }
      setBasemap(style === null ? "offline" : "online");

      const map = new MapLibreMap({
        container,
        style: style ?? offlineStyle(edge),
        center: [0, 25],
        zoom: 1,
        attributionControl: false,
      });
      mapRef.current = map;
      map.addControl(new CollapsedAttributionControl({ compact: true }));
      map.addControl(new NavigationControl({ showCompass: false }), "top-right");

      map.on("style.load", () => {
        installReferenceLayer(map, accentRef.current, edge, referencesRef.current);
        installLayers(map, edge, kindsRef.current);
        installSignalLayers(map, edge, signalSamplesRef.current !== null);
        installPositionLayers(map, accentRef.current, edge, positionNodesRef.current.length > 0);
        readyRef.current = true;
        drawnRef.current = {};
        highlight(map, kindsRef.current, selectedRef.current);
      });

      // Once the operator has moved the map themselves, the auto-frame below must never take the
      // view back off them.
      map.on("movestart", (event) => {
        if (event.originalEvent !== undefined) {
          targetFramedRef.current = true;
          positionFramedRef.current = true;
          signalFramedRef.current = true;
        }
      });

      map.on("click", (event) => select(hitTarget(map, event)));

      // One hit test against the layers that exist, rather than MapLibre's per-layer
      // enter/leave listeners: the layer set follows the wires, and a delegated listener
      // outlives the layer it names.
      map.on("mousemove", (event) => {
        map.getCanvas().style.cursor = hitTarget(map, event) === null ? "" : "pointer";
      });
    })();

    return () => {
      disposed = true;
      readyRef.current = false;
      mapRef.current?.remove();
      mapRef.current = null;
    };
  }, []);

  useEffect(() => {
    const draw = (): void => {
      const map = mapRef.current;
      if (map === null || !readyRef.current) {
        return;
      }
      const stations = useDecodedStore.getState().stations;
      const now = Date.now();
      const next = { ...countsRef.current };
      let changed = false;

      const positionSources = usePositionStore.getState().sources;
      const tracks = positionNodesRef.current.map((node) => ({
        node,
        samples: positionSources[node]?.history ?? EMPTY_POSITION_HISTORY,
        active: positionSources[node]?.fix != null,
      }));
      const positionKey = tracks
        .map(
          ({ node, samples, active: live }) =>
            `${node}:${samples.length}:${samples.at(-1)?.receivedAt ?? 0}:${live ? "live" : "stale"}`,
        )
        .join("|");
      if (positionKey !== positionDrawnRef.current) {
        positionDrawnRef.current = positionKey;
        const collection = updatePositionSources(
          map.getSource<GeoJSONSource>(POSITION_SOURCE),
          map.getSource<GeoJSONSource>(POSITION_ROUTE_SOURCE),
          tracks,
        );
        setPositionCount(collection.points.features.length);
        framePositionOnce(map, collection.points, positionFramedRef);
      }

      const surveySamples = signalSamplesRef.current;
      if (surveySamples !== null && surveySamples !== signalDrawnRef.current) {
        signalDrawnRef.current = surveySamples;
        const collection = updateSignalSource(
          map.getSource<GeoJSONSource>(SIGNAL_SOURCE),
          surveySamples,
        );
        setSignalCount(collection.features.length);
        frameSignalOnce(map, collection, signalFramedRef);
      }

      for (const kind of kindsRef.current) {
        const rows = stations[kind] ?? EMPTY_STATIONS;
        if (rows === drawnRef.current[kind]) {
          continue;
        }
        drawnRef.current[kind] = rows;
        const collection = targetCollection(rows, now);
        // `setData` resolves when the tile re-parse lands; the next tick is our only consumer.
        void map.getSource<GeoJSONSource>(sourceId(kind))?.setData(collection);
        next[kind] = collection.features.length;
        changed = true;
        frameTargetsOnce(map, collection, targetFramedRef);
      }

      if (changed && !sameCounts(countsRef.current, next)) {
        countsRef.current = next;
        setCounts(next);
      }

      const selected = selectedRef.current;
      if (selected !== null) {
        const current = findDetail(selected.kind, selected.id);
        setDetail((previous) =>
          current === null || previous === null || current.lastSeen !== previous.lastSeen
            ? current
            : previous,
        );
        if (current === null) {
          selectedRef.current = null;
          highlight(map, kindsRef.current, null);
        }
      }
    };

    const drawTimer = setInterval(draw, DRAW_TICK_MS);
    // The store's age-out is global, so the horizon this panel picks is the one every target
    // view gets; `TARGET_MAX_AGE_MS` is deliberately generous for that reason.
    const ageTimer = setInterval(
      () => useDecodedStore.getState().ageOut(TARGET_MAX_AGE_MS),
      AGE_OUT_INTERVAL_MS,
    );
    return () => {
      clearInterval(drawTimer);
      clearInterval(ageTimer);
    };
  }, []);

  // The map's own gestures are handed over with the face: an inactive one is a picture, and the
  // wheel across it belongs to the canvas camera. Re-run on every render of an active map too —
  // the handlers only exist once the style has landed, which is later than the first effect.
  useEffect(() => {
    const map = mapRef.current;
    if (map === null) {
      return;
    }
    for (const handler of [
      map.scrollZoom,
      map.dragPan,
      map.boxZoom,
      map.doubleClickZoom,
      map.touchZoomRotate,
      map.keyboard,
    ]) {
      if (active) {
        handler.enable();
      } else {
        handler.disable();
      }
    }
  }, [active, basemap]);

  // Rewiring the node changes what it plots. The key, not the array, is the dependency: `kinds`
  // is rebuilt on every render, and a rebuilt layer stack twice a second is not what a wire
  // change costs.
  const kindsKey = kinds.join(" ");
  useEffect(() => {
    const map = mapRef.current;
    if (map === null || !readyRef.current) {
      return;
    }
    const wired = mapKindsOf(kindsKey.split(" "));
    installLayers(map, edgeRef.current, wired);
    drawnRef.current = {};
    const selected = selectedRef.current;
    if (selected !== null && !wired.includes(selected.kind)) {
      selectedRef.current = null;
      setDetail(null);
    }
    // A rebuilt dot layer carries default paint, so a selection that survived the rewire has to
    // be drawn onto it again.
    highlight(map, wired, selectedRef.current);
  }, [kindsKey]);

  const referencesKey = JSON.stringify(references);
  useEffect(() => {
    const map = mapRef.current;
    if (map === null || !readyRef.current) {
      return;
    }
    const positions = JSON.parse(referencesKey) as [number, number][];
    installReferenceLayer(map, accentRef.current, edgeRef.current, positions);
  }, [referencesKey]);

  const positionNodesKey = positionNodes.join(" ");
  useEffect(() => {
    const map = mapRef.current;
    if (map === null || !readyRef.current) {
      return;
    }
    installPositionLayers(map, accentRef.current, edgeRef.current, positionNodesKey !== "");
    positionDrawnRef.current = "\0";
  }, [positionNodesKey]);

  const signalEnabled = signalSamples !== undefined;
  useEffect(() => {
    const map = mapRef.current;
    if (map === null || !readyRef.current) {
      return;
    }
    installSignalLayers(map, edgeRef.current, signalEnabled);
    signalDrawnRef.current = null;
    if (!signalEnabled) {
      setSignalCount(0);
    }
  }, [signalEnabled]);

  return (
    <div className={`relative ${className ?? "h-[min(60dvh,28rem)] min-h-64 w-full"}`}>
      {/* Sized in flow, not `absolute inset-0`: MapLibre stamps `maplibregl-map` onto this
          element, and its stylesheet's unlayered `position: relative` beats Tailwind's layered
          utilities — `inset-0` then anchors to nothing and the box collapses to zero height. */}
      <div ref={containerRef} className="h-full w-full bg-bg" />

      <div className="pointer-events-none absolute top-2 left-2 flex flex-col items-start gap-1">
        <div className="flex flex-col gap-1 rounded border border-line bg-bg/85 px-2 py-1.5">
          {kinds.map((kind) => (
            <div key={kind} className="flex items-center gap-2 font-mono text-[10px] tabular-nums">
              <span
                className="inline-block h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: KIND_STYLE[kind].color }}
              />
              <span className="text-ink-dim">{KIND_STYLE[kind].title}</span>
              <span className="ml-auto text-ink">{counts[kind]}</span>
            </div>
          ))}
          {positionNodes.length > 0 && (
            <div className="flex items-center gap-2 font-mono text-[10px] tabular-nums">
              <span className="inline-block h-2 w-2 shrink-0 rounded-full bg-accent" />
              <span className="text-ink-dim">GPS trail</span>
              <span className="ml-auto text-ink">{positionCount}</span>
            </div>
          )}
          {signalSamples !== undefined && (
            <div className="flex min-w-36 flex-col gap-1 font-mono text-[10px] tabular-nums">
              <div className="flex items-center justify-between gap-3">
                <span className="text-ink-dim">Signal cells</span>
                <span className="text-ink">{signalCount}</span>
              </div>
              <div
                className="h-1.5 w-full rounded-full"
                style={{
                  background:
                    "linear-gradient(to right, #231942, #5e2b83, #b33f62, #ef8354, #f6d365)",
                }}
              />
              <div className="flex justify-between text-ink-faint">
                <span>{SIGNAL_MIN_DBFS} dBFS</span>
                <span>{SIGNAL_MAX_DBFS} dBFS</span>
              </div>
            </div>
          )}
        </div>
        {basemap === "offline" && (
          <div className="rounded border border-line bg-bg/85 px-2 py-1 font-mono text-[10px] text-ink-dim">
            basemap unavailable (offline)
          </div>
        )}
      </div>

      {detail !== null && (
        <div className="absolute inset-x-2 bottom-2 rounded border border-line bg-panel/95 md:inset-x-auto md:right-2 md:w-64">
          <div className="flex items-center justify-between gap-2 border-b border-line px-2 py-1">
            <span
              className="truncate font-mono text-sm"
              style={{ color: KIND_STYLE[detail.kind].color }}
            >
              {detail.label}
            </span>
            <Button
              type="button"
              className="shrink-0 px-1 font-mono text-xs text-ink-dim hover:text-ink"
              onClick={() => {
                selectedRef.current = null;
                setDetail(null);
                highlight(mapRef.current, kinds, null);
              }}
              aria-label="Clear target selection"
            >
              ×
            </Button>
          </div>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 px-2 py-1.5">
            {detail.rows.map(([label, value]) => (
              <div key={label} className="col-span-2 grid grid-cols-subgrid">
                <dt className="text-[10px] text-ink-dim uppercase tracking-wider">{label}</dt>
                <dd className="truncate text-right font-mono text-xs tabular-nums text-ink">
                  {value}
                </dd>
              </div>
            ))}
          </dl>
          <div className="border-t border-line px-2 py-1 font-mono text-[10px] tabular-nums text-ink-dim">
            {formatMhz(detail.freqHz)} · last seen {formatUtc(detail.lastSeen)}
          </div>
        </div>
      )}
    </div>
  );
}

const EMPTY_STATIONS: readonly Target[] = Object.freeze([]);
const EMPTY_POSITION_HISTORY: readonly PositionSample[] = Object.freeze([]);
const POSITION_SOURCE = "station-position-history";
const POSITION_ROUTE_SOURCE = "station-position-route";
const POSITION_LAYERS = ["station-position-heat", "station-position-route", "station-position-fix"];
const SIGNAL_SOURCE = "signal-survey";
const SIGNAL_LAYERS = ["signal-survey-heat", "signal-survey-points"];

interface PositionFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { latest: boolean; at: number };
}

export interface PositionCollection {
  type: "FeatureCollection";
  features: PositionFeature[];
}

export interface PositionRouteCollection {
  type: "FeatureCollection";
  features: {
    type: "Feature";
    geometry: { type: "LineString"; coordinates: [number, number][] };
    properties: Record<string, never>;
  }[];
}

export function positionCollection(
  tracks: readonly { samples: readonly PositionSample[]; active: boolean }[],
): {
  points: PositionCollection;
  route: PositionRouteCollection;
} {
  const features = tracks.flatMap(({ samples, active }) =>
    samples.map((sample, index) => ({
      type: "Feature" as const,
      geometry: {
        type: "Point" as const,
        coordinates: [sample.longitude, sample.latitude] as [number, number],
      },
      properties: { latest: active && index === samples.length - 1, at: sample.receivedAt },
    })),
  );
  return {
    points: { type: "FeatureCollection", features },
    route: {
      type: "FeatureCollection",
      features: tracks.flatMap(({ samples }) =>
        samples.length < 2
          ? []
          : [
              {
                type: "Feature" as const,
                geometry: {
                  type: "LineString" as const,
                  coordinates: unwrapTrail(
                    samples.map(
                      (sample) => [sample.longitude, sample.latitude] as [number, number],
                    ),
                  ),
                },
                properties: {},
              },
            ],
      ),
    },
  };
}

export function updatePositionSources(
  pointsSource: Pick<GeoJSONSource, "setData"> | undefined,
  routeSource: Pick<GeoJSONSource, "setData"> | undefined,
  tracks: readonly { samples: readonly PositionSample[]; active: boolean }[],
): { points: PositionCollection; route: PositionRouteCollection } {
  const collection = positionCollection(tracks);
  void pointsSource?.setData(collection.points);
  void routeSource?.setData(collection.route);
  return collection;
}

interface SignalFeature {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { level: number; observations: number };
}

export interface SignalCollection {
  type: "FeatureCollection";
  features: SignalFeature[];
}

export function signalCollection(samples: readonly SignalSurveySample[]): SignalCollection {
  return {
    type: "FeatureCollection",
    features: samples.map((sample) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [sample.longitude, sample.latitude] },
      properties: { level: sample.levelDbfs, observations: sample.observations },
    })),
  };
}

export function updateSignalSource(
  source: Pick<GeoJSONSource, "setData"> | undefined,
  samples: readonly SignalSurveySample[],
): SignalCollection {
  const collection = signalCollection(samples);
  void source?.setData(collection);
  return collection;
}

const LAYER_PARTS = ["dot", "heading", "label"] as const;

const LAYER_KIND: ReadonlyMap<string, MapKind> = new Map(
  MAP_KINDS.flatMap((kind) => LAYER_PARTS.map((part) => [layerId(kind, part), kind])),
);

/** The topmost target under the pointer. Layers the style does not carry are filtered out
 * first: an unwired kind has none at all, a kind whose course icon could not be rasterised has
 * no heading layer, and querying a layer that is not there is an error. */
function hitTarget(map: MapLibreMap, event: MapMouseEvent): { kind: MapKind; id: string } | null {
  const layers = [...LAYER_KIND.keys()].filter((id) => map.getLayer(id) !== undefined);
  if (layers.length === 0) {
    return null;
  }
  const { x, y } = event.point;
  const hit = map.queryRenderedFeatures(
    [
      [x - HIT_SLOP_PX, y - HIT_SLOP_PX],
      [x + HIT_SLOP_PX, y + HIT_SLOP_PX],
    ],
    { layers },
  )[0];
  const kind = hit === undefined ? undefined : LAYER_KIND.get(hit.layer.id);
  const id: unknown = hit?.properties.id;
  return kind === undefined || typeof id !== "string" ? null : { kind, id };
}

/** `null` = the basemap could not be reached, so the caller falls back to the offline backdrop.
 * Pre-fetching rather than handing MapLibre the URL is what makes that fallback possible: a
 * style that 404s or times out inside MapLibre never fires `style.load`, and the target layers
 * would never be installed at all. */
async function fetchStyle(): Promise<MapStyle | null> {
  try {
    const response = await fetch(BASEMAP_STYLE_URL, {
      signal: AbortSignal.timeout(BASEMAP_TIMEOUT_MS),
    });
    if (!response.ok) {
      return null;
    }
    return (await response.json()) as MapStyle;
  } catch {
    return null;
  }
}

function offlineStyle(background: string): MapStyle {
  return {
    version: 8,
    sources: {},
    layers: [{ id: "backdrop", type: "background", paint: { "background-color": background } }],
  };
}

/** The map's layer stack, rebuilt from scratch for exactly the wired kinds. Taking the whole
 * stack down first is what keeps a rewire cheap to reason about: the alternative is a diff whose
 * insertion order has to reproduce the dots-then-labels rule below. */
function installLayers(map: MapLibreMap, edge: string, kinds: readonly MapKind[]): void {
  for (const kind of MAP_KINDS) {
    for (const part of LAYER_PARTS) {
      if (map.getLayer(layerId(kind, part)) !== undefined) {
        map.removeLayer(layerId(kind, part));
      }
    }
    if (map.getSource(sourceId(kind)) !== undefined) {
      map.removeSource(sourceId(kind));
    }
  }

  for (const kind of kinds) {
    const { color } = KIND_STYLE[kind];
    map.addSource(sourceId(kind), { type: "geojson", data: EMPTY_COLLECTION });

    map.addLayer({
      id: layerId(kind, "dot"),
      type: "circle",
      source: sourceId(kind),
      paint: {
        "circle-radius": 4,
        "circle-color": color,
        // The stroke is the theme background, so a target stays legible over OpenFreeMap's light
        // tiles and over the dark offline backdrop without two colour schemes.
        "circle-stroke-color": edge,
        "circle-stroke-width": 1,
      },
    });

    // The heading symbol only exists if we could rasterise one; without it the map keeps its
    // dots rather than asking MapLibre for an image that is not there. Images outlive the layer
    // stack, so a rewire reuses the one already registered.
    const icon = `${sourceId(kind)}-icon`;
    if (!map.hasImage(icon)) {
      const image = KIND_ICON[kind](color, edge);
      if (image === null) {
        continue;
      }
      map.addImage(icon, image, { pixelRatio: ICON_SCALE });
    }
    map.addLayer({
      id: layerId(kind, "heading"),
      type: "symbol",
      source: sourceId(kind),
      filter: ["has", "heading"],
      layout: {
        "icon-image": icon,
        "icon-rotate": ["get", "heading"],
        "icon-rotation-alignment": "map",
        "icon-allow-overlap": true,
        "icon-ignore-placement": true,
      },
    });
  }

  for (const kind of kinds) {
    map.addLayer({
      id: layerId(kind, "label"),
      type: "symbol",
      source: sourceId(kind),
      layout: {
        "text-field": ["get", "label"],
        "text-font": ["Noto Sans Regular"],
        "text-size": 11,
        "text-anchor": "top",
        "text-offset": [0, LABEL_OFFSET_EM[kind]],
        "text-optional": true,
      },
      paint: {
        "text-color": KIND_STYLE[kind].color,
        "text-halo-color": edge,
        "text-halo-width": 1.4,
      },
    });
  }
}

function installSignalLayers(map: MapLibreMap, edge: string, enabled: boolean): void {
  for (const layer of SIGNAL_LAYERS) {
    if (map.getLayer(layer) !== undefined) {
      map.removeLayer(layer);
    }
  }
  if (map.getSource(SIGNAL_SOURCE) !== undefined) {
    map.removeSource(SIGNAL_SOURCE);
  }
  if (!enabled) {
    return;
  }

  map.addSource(SIGNAL_SOURCE, {
    type: "geojson",
    data: { type: "FeatureCollection", features: [] },
  });
  map.addLayer({
    id: SIGNAL_LAYERS[0] ?? "signal-survey-heat",
    type: "heatmap",
    source: SIGNAL_SOURCE,
    maxzoom: 17,
    paint: {
      "heatmap-weight": [
        "interpolate",
        ["linear"],
        ["get", "level"],
        SIGNAL_MIN_DBFS,
        0.05,
        SIGNAL_MAX_DBFS,
        1,
      ],
      "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 0.35, 15, 1.25],
      "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 3, 15, 20],
      "heatmap-opacity": ["interpolate", ["linear"], ["zoom"], 13, 0.8, 17, 0.2],
      "heatmap-color": [
        "interpolate",
        ["linear"],
        ["heatmap-density"],
        0,
        "rgba(35,25,66,0)",
        0.15,
        "#231942",
        0.35,
        "#5e2b83",
        0.55,
        "#b33f62",
        0.75,
        "#ef8354",
        1,
        "#f6d365",
      ],
    },
  });
  map.addLayer({
    id: SIGNAL_LAYERS[1] ?? "signal-survey-points",
    type: "circle",
    source: SIGNAL_SOURCE,
    minzoom: 13,
    paint: {
      "circle-radius": ["interpolate", ["linear"], ["zoom"], 13, 2, 17, 6],
      "circle-opacity": ["interpolate", ["linear"], ["zoom"], 13, 0, 15, 0.9],
      "circle-color": [
        "interpolate",
        ["linear"],
        ["get", "level"],
        SIGNAL_MIN_DBFS,
        "#231942",
        -95,
        "#5e2b83",
        -70,
        "#b33f62",
        -45,
        "#ef8354",
        SIGNAL_MAX_DBFS,
        "#f6d365",
      ],
      "circle-stroke-color": edge,
      "circle-stroke-width": 1,
    },
  });
}

function installPositionLayers(
  map: MapLibreMap,
  accent: string,
  edge: string,
  enabled: boolean,
): void {
  for (const layer of POSITION_LAYERS) {
    if (map.getLayer(layer) !== undefined) {
      map.removeLayer(layer);
    }
  }
  for (const source of [POSITION_SOURCE, POSITION_ROUTE_SOURCE]) {
    if (map.getSource(source) !== undefined) {
      map.removeSource(source);
    }
  }
  if (!enabled) {
    return;
  }
  map.addSource(POSITION_SOURCE, {
    type: "geojson",
    data: { type: "FeatureCollection", features: [] },
  });
  map.addSource(POSITION_ROUTE_SOURCE, {
    type: "geojson",
    data: { type: "FeatureCollection", features: [] },
  });
  map.addLayer({
    id: POSITION_LAYERS[0] ?? "station-position-heat",
    type: "heatmap",
    source: POSITION_SOURCE,
    maxzoom: 16,
    paint: {
      "heatmap-weight": 1,
      "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 0.5, 14, 2],
      "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 3, 14, 22],
      "heatmap-opacity": ["interpolate", ["linear"], ["zoom"], 10, 0.65, 16, 0.2],
      "heatmap-color": [
        "interpolate",
        ["linear"],
        ["heatmap-density"],
        0,
        "rgba(0,0,0,0)",
        0.35,
        accent,
        1,
        "#ef6262",
      ],
    },
  });
  map.addLayer({
    id: POSITION_LAYERS[1] ?? "station-position-route",
    type: "line",
    source: POSITION_ROUTE_SOURCE,
    paint: { "line-color": accent, "line-width": 2, "line-opacity": 0.8 },
  });
  map.addLayer({
    id: POSITION_LAYERS[2] ?? "station-position-fix",
    type: "circle",
    source: POSITION_SOURCE,
    filter: ["==", ["get", "latest"], true],
    paint: {
      "circle-radius": 6,
      "circle-color": accent,
      "circle-stroke-color": edge,
      "circle-stroke-width": 2,
    },
  });
}

function highlight(
  map: MapLibreMap | null,
  kinds: readonly MapKind[],
  selected: { kind: MapKind; id: string } | null,
): void {
  if (map === null) {
    return;
  }
  for (const kind of kinds) {
    if (map.getLayer(layerId(kind, "dot")) === undefined) {
      continue;
    }
    const active = selected !== null && selected.kind === kind ? selected.id : null;
    map.setPaintProperty(
      layerId(kind, "dot"),
      "circle-stroke-width",
      active === null ? 1 : ["case", ["==", ["get", "id"], active], 3, 1],
    );
    map.setPaintProperty(
      layerId(kind, "dot"),
      "circle-radius",
      active === null ? 4 : ["case", ["==", ["get", "id"], active], 6, 4],
    );
  }
}

/** Opens on the targets instead of on the whole globe, once, when the first ones land. */
function frame(map: Pick<MapLibreMap, "fitBounds">, collection: TargetCollection): void {
  let west = Number.POSITIVE_INFINITY;
  let south = Number.POSITIVE_INFINITY;
  let east = Number.NEGATIVE_INFINITY;
  let north = Number.NEGATIVE_INFINITY;
  for (const feature of collection.features) {
    const [lon, lat] = feature.geometry.coordinates;
    west = Math.min(west, lon);
    east = Math.max(east, lon);
    south = Math.min(south, lat);
    north = Math.max(north, lat);
  }
  map.fitBounds(
    [
      [west, south],
      [east, north],
    ],
    { padding: 56, maxZoom: 9, duration: 0 },
  );
}

interface FrameFlag {
  current: boolean;
}

export function frameTargetsOnce(
  map: Pick<MapLibreMap, "fitBounds">,
  collection: TargetCollection,
  framed: FrameFlag,
): void {
  if (framed.current || collection.features.length === 0) {
    return;
  }
  framed.current = true;
  frame(map, collection);
}

export function framePositionOnce(
  map: Pick<MapLibreMap, "fitBounds">,
  collection: PositionCollection,
  framed: FrameFlag,
): void {
  if (framed.current || collection.features.length === 0) {
    return;
  }
  framed.current = true;
  framePoints(
    map,
    collection.features.map((feature) => feature.geometry.coordinates),
  );
}

export function frameSignalOnce(
  map: Pick<MapLibreMap, "fitBounds">,
  collection: SignalCollection,
  framed: FrameFlag,
): void {
  if (framed.current || collection.features.length === 0) {
    return;
  }
  framed.current = true;
  framePoints(
    map,
    collection.features.map((feature) => feature.geometry.coordinates),
  );
}

function framePoints(
  map: Pick<MapLibreMap, "fitBounds">,
  coordinates: readonly [number, number][],
): void {
  const bounds = trailBounds(coordinates);
  if (bounds === null) {
    return;
  }
  map.fitBounds(bounds, { padding: 56, maxZoom: 14, duration: 0 });
}

const ICON_SCALE = 2;
const ARROW_PX = 18;
const PLANE_PX = 26;
const SHIP_PX = 22;

/** The raster a heading-bearing target draws: kind read off the shape before the colour
 * ( — every colour paired with a marker). */
const KIND_ICON: Record<MapKind, (color: string, edge: string) => ImageData | null> = {
  adsb: planeImage,
  ais: shipImage,
  aprs: arrowImage,
};

/** Ems the label sits below the position — past each kind's symbol, not through it. */
const LABEL_OFFSET_EM: Record<MapKind, number> = { adsb: 1.3, ais: 1.1, aprs: 0.7 };

/** `null` when the browser gives us no 2D context — the map then draws dots only. */
function rasterize(px: number, draw: (ctx: CanvasRenderingContext2D) => void): ImageData | null {
  const canvas = document.createElement("canvas");
  canvas.width = px * ICON_SCALE;
  canvas.height = px * ICON_SCALE;
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return null;
  }
  ctx.scale(ICON_SCALE, ICON_SCALE);
  draw(ctx);
  return ctx.getImageData(0, 0, canvas.width, canvas.height);
}

/** Traces the starboard half of a north-facing outline and its port mirror, so symmetry is
 * stated once instead of maintained twice. */
function silhouette(
  ctx: CanvasRenderingContext2D,
  px: number,
  starboard: readonly (readonly [number, number])[],
): void {
  const outline = [...starboard, ...[...starboard].reverse().map(([x, y]) => [px - x, y] as const)];
  ctx.beginPath();
  outline.forEach(([x, y], index) => (index === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
  ctx.closePath();
}

/** The edge stroke goes under the fill: half the line survives as a halo against the basemap
 * without thinning the silhouette. */
function paint(ctx: CanvasRenderingContext2D, color: string, edge: string): void {
  ctx.strokeStyle = edge;
  ctx.lineWidth = 1;
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.fillStyle = color;
  ctx.fill();
}

/** A course indicator pointing north at rotation 0, drawn clear of the 4 px dot so the two read
 * as one symbol. */
function arrowImage(color: string, edge: string): ImageData | null {
  return rasterize(ARROW_PX, (ctx) => {
    const mid = ARROW_PX / 2;
    ctx.beginPath();
    ctx.moveTo(mid, 1.5);
    ctx.lineTo(mid + 3.5, 8);
    ctx.lineTo(mid, 6.5);
    ctx.lineTo(mid - 3.5, 8);
    ctx.closePath();
    paint(ctx, color, edge);
  });
}

function planeImage(color: string, edge: string): ImageData | null {
  return rasterize(PLANE_PX, (ctx) => {
    silhouette(ctx, PLANE_PX, [
      [13, 1.6],
      [14.4, 4.2],
      [14.4, 9.4],
      [24.4, 14.2],
      [24.4, 16.2],
      [14.4, 13.6],
      [14.4, 19.2],
      [18.6, 21.8],
      [18.6, 23.4],
      [13.6, 22.4],
      [13, 23.6],
    ]);
    paint(ctx, color, edge);
  });
}

/** A hull seen from above, bow north — the AIS symbol every chartplotter taught. */
function shipImage(color: string, edge: string): ImageData | null {
  return rasterize(SHIP_PX, (ctx) => {
    silhouette(ctx, SHIP_PX, [
      [11, 1.8],
      [15.6, 7.4],
      [15.6, 17.2],
      [14.2, 19.8],
      [11, 19.8],
    ]);
    paint(ctx, color, edge);
  });
}

const REFERENCE_ID = "station-reference";

/** A ring and a centre dot: a fix, not a mover — no heading, no dot layer, no selection. */
function stationImage(color: string, edge: string): ImageData | null {
  const PX = 20;
  return rasterize(PX, (ctx) => {
    const mid = PX / 2;
    ctx.beginPath();
    ctx.arc(mid, mid, 6, 0, Math.PI * 2);
    ctx.strokeStyle = edge;
    ctx.lineWidth = 3.5;
    ctx.stroke();
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.6;
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(mid, mid, 1.7, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.fill();
  });
}

/** Where the wired decoders decode *from*: an ADS-B channel's CPR reference is the antenna's
 * own fix, worth seeing among the targets it anchors. A landmark, not a target — kept out of
 * the hit test and installed once, before the target layers, so they stack above it. */
function installReferenceLayer(
  map: MapLibreMap,
  accent: string,
  edge: string,
  positions: readonly (readonly [number, number])[],
): void {
  if (map.getSource(REFERENCE_ID) === undefined) {
    if (!map.hasImage(REFERENCE_ID)) {
      const image = stationImage(accent, edge);
      if (image === null) {
        return;
      }
      map.addImage(REFERENCE_ID, image, { pixelRatio: ICON_SCALE });
    }
    map.addSource(REFERENCE_ID, { type: "geojson", data: referenceCollection(positions) });
    map.addLayer({
      id: REFERENCE_ID,
      type: "symbol",
      source: REFERENCE_ID,
      layout: {
        "icon-image": REFERENCE_ID,
        "icon-allow-overlap": true,
        "icon-ignore-placement": true,
      },
    });
    return;
  }
  void map.getSource<GeoJSONSource>(REFERENCE_ID)?.setData(referenceCollection(positions));
}

function findDetail(kind: MapKind, id: string): TargetDetail | null {
  const station = useDecodedStore.getState().stations[kind]?.find((row) => row.id === id);
  return station === undefined ? null : targetDetail(station);
}

function sameCounts(a: Counts, b: Counts): boolean {
  return MAP_KINDS.every((kind) => a[kind] === b[kind]);
}

/** Reads a theme token off the live element so the map follows the app's palette rather than
 * pinning a second copy of it. Painted down to sRGB hex through a pixel because MapLibre's
 * style spec takes CSS Color 3 only and *drops the whole layer* handed the tokens' `oklch(...)`
 * — and canvas `fillStyle` reads back unconverted, so painting is the only conversion. */
function themeColor(element: Element, token: string, fallback: string): string {
  const value = getComputedStyle(element).getPropertyValue(token).trim();
  if (value === "") {
    return fallback;
  }
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return fallback;
  }
  ctx.fillStyle = value;
  ctx.fillRect(0, 0, 1, 1);
  const [r = 0, g = 0, b = 0] = ctx.getImageData(0, 0, 1, 1).data;
  return `#${[r, g, b].map((channel) => channel.toString(16).padStart(2, "0")).join("")}`;
}

/** Absolute UTC, not "12 s ago": a wall clock does not need a re-render to stay true. */
function formatUtc(ms: number): string {
  return `${new Date(ms).toISOString().slice(11, 19)}Z`;
}
