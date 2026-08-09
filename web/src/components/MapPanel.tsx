// Aircraft (ADS-B), ships (AIS) and APRS stations on one MapLibre map (PLAN §10 Maps, §13 P2).
// Targets come from the decoded store, never from TanStack Query — this is the high-rate plane.
// There is deliberately no React element per target: MapLibre owns one GeoJSON source per kind
// and gets a `setData` on the `DRAW_TICK_MS` tick, so a thousand aircraft cost three source
// updates every 500 ms rather than a thousand components.
import "maplibre-gl/dist/maplibre-gl.css";
import {
  type GeoJSONSource,
  Map as MapLibreMap,
  type MapOptions,
  NavigationControl,
} from "maplibre-gl";
import { useEffect, useRef, useState } from "react";
import { useDecodedStore } from "../lib/decoded";
import {
  AGE_OUT_INTERVAL_MS,
  DRAW_TICK_MS,
  KIND_STYLE,
  layerId,
  MAP_KINDS,
  type MapKind,
  sourceId,
  TARGET_MAX_AGE_MS,
  type Target,
  type TargetCollection,
  type TargetDetail,
  targetCollection,
  targetDetail,
} from "../lib/map/layers";
import { formatMhz } from "./format";

/** PLAN §10: OpenFreeMap vector tiles — free, no API key, no usage cap. A self-hosted PMTiles
 * basemap replaces this URL when the server grows one; nothing else here changes. */
const BASEMAP_STYLE_URL = "https://tiles.openfreemap.org/styles/liberty";

/** A field Pi has no internet and must not wait on one: if the style has not arrived by then,
 * the map opens on the offline backdrop instead of hanging with a blank canvas. */
const BASEMAP_TIMEOUT_MS = 6_000;

/** Half-size of the click/tap hit box in pixels — a 4 px dot is not a touch target. */
const HIT_SLOP_PX = 9;

type MapStyle = Exclude<NonNullable<MapOptions["style"]>, string>;
type Counts = Record<MapKind, number>;

const EMPTY_COLLECTION: TargetCollection = { type: "FeatureCollection", features: [] };
const ZERO_COUNTS: Counts = { adsb: 0, ais: 0, aprs: 0 };

export function MapPanel({ className }: { className?: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const readyRef = useRef(false);
  // Last station array published per kind. Comparing identity (the store hands out a new array
  // only when that kind changed) keeps an idle map from re-serialising GeoJSON twice a second.
  const drawnRef = useRef<Partial<Record<MapKind, readonly Target[]>>>({});
  const selectedRef = useRef<{ kind: MapKind; id: string } | null>(null);
  const framedRef = useRef(false);

  const countsRef = useRef<Counts>(ZERO_COUNTS);

  const [counts, setCounts] = useState<Counts>(ZERO_COUNTS);
  const [detail, setDetail] = useState<TargetDetail | null>(null);
  const [basemap, setBasemap] = useState<"pending" | "online" | "offline">("pending");

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) {
      return;
    }
    let disposed = false;

    const select = (next: { kind: MapKind; id: string } | null): void => {
      selectedRef.current = next;
      setDetail(next === null ? null : findDetail(next.kind, next.id));
      highlight(mapRef.current, next);
    };

    void (async () => {
      // One token does double duty: the offline backdrop, and the outline that keeps every
      // target readable against whichever basemap is under it.
      const edge = themeColor(container, "--color-bg", "#0b0e14");
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
        attributionControl: { compact: true },
      });
      mapRef.current = map;
      map.addControl(new NavigationControl({ showCompass: false }), "top-right");

      map.on("style.load", () => {
        installLayers(map, edge);
        readyRef.current = true;
        drawnRef.current = {};
        highlight(map, selectedRef.current);
      });

      // Once the operator has moved the map themselves, the auto-frame below must never take the
      // view back off them.
      map.on("movestart", (event) => {
        if (event.originalEvent !== undefined) {
          framedRef.current = true;
        }
      });

      map.on("click", (event) => {
        const { x, y } = event.point;
        const hit = map.queryRenderedFeatures(
          [
            [x - HIT_SLOP_PX, y - HIT_SLOP_PX],
            [x + HIT_SLOP_PX, y + HIT_SLOP_PX],
          ],
          // A kind whose course icon could not be rasterised has no heading layer, and querying
          // a layer the style does not have is an error.
          { layers: allLayerIds().filter((id) => map.getLayer(id) !== undefined) },
        )[0];
        const kind = hit === undefined ? undefined : LAYER_KIND.get(hit.layer.id);
        const id: unknown = hit?.properties.id;
        select(kind === undefined || typeof id !== "string" ? null : { kind, id });
      });

      for (const kind of MAP_KINDS) {
        const clickable = [layerId(kind, "dot"), layerId(kind, "label")];
        map.on("mouseenter", clickable, () => {
          map.getCanvas().style.cursor = "pointer";
        });
        map.on("mouseleave", clickable, () => {
          map.getCanvas().style.cursor = "";
        });
      }
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

      for (const kind of MAP_KINDS) {
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
        if (!framedRef.current && collection.features.length > 0) {
          framedRef.current = true;
          frame(map, collection);
        }
      }

      if (changed && !sameCounts(countsRef.current, next)) {
        countsRef.current = next;
        setCounts(next);
      }

      const selected = selectedRef.current;
      if (selected !== null) {
        const current = findDetail(selected.kind, selected.id);
        // A target that aged out of the store takes its selection with it.
        setDetail((previous) =>
          current === null || previous === null || current.lastSeen !== previous.lastSeen
            ? current
            : previous,
        );
        if (current === null) {
          selectedRef.current = null;
          highlight(map, null);
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

  return (
    <div className={`relative ${className ?? "h-[min(60dvh,28rem)] min-h-64 w-full"}`}>
      <div ref={containerRef} className="absolute inset-0 bg-bg" />

      <div className="pointer-events-none absolute top-2 left-2 flex flex-col items-start gap-1">
        <div className="flex flex-col gap-1 rounded border border-line bg-bg/85 px-2 py-1.5">
          {MAP_KINDS.map((kind) => (
            <div key={kind} className="flex items-center gap-2 font-mono text-[10px] tabular-nums">
              <span
                className="inline-block h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: KIND_STYLE[kind].color }}
              />
              <span className="text-ink-dim">{KIND_STYLE[kind].title}</span>
              <span className="ml-auto text-ink">{counts[kind]}</span>
            </div>
          ))}
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
            <button
              type="button"
              className="shrink-0 px-1 font-mono text-xs text-ink-dim hover:text-ink"
              onClick={() => {
                selectedRef.current = null;
                setDetail(null);
                highlight(mapRef.current, null);
              }}
              aria-label="Clear target selection"
            >
              ×
            </button>
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

const LAYER_KIND: ReadonlyMap<string, MapKind> = new Map(
  MAP_KINDS.flatMap((kind) =>
    (["dot", "heading", "label"] as const).map((part) => [layerId(kind, part), kind]),
  ),
);

function allLayerIds(): string[] {
  return [...LAYER_KIND.keys()];
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
  // No `glyphs` entry on purpose: with no glyph server MapLibre rasterises label glyphs locally
  // (TinySDF), so target labels still draw with no network at all.
  return {
    version: 8,
    sources: {},
    layers: [{ id: "backdrop", type: "background", paint: { "background-color": background } }],
  };
}

function installLayers(map: MapLibreMap, edge: string): void {
  for (const kind of MAP_KINDS) {
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

    // A course indicator only exists if we could rasterise one; without it the map keeps its
    // dots rather than asking MapLibre for an image that is not there.
    const arrow = `${sourceId(kind)}-arrow`;
    const image = arrowImage(color, edge);
    if (image === null) {
      continue;
    }
    map.addImage(arrow, image, { pixelRatio: ARROW_SCALE });
    map.addLayer({
      id: layerId(kind, "heading"),
      type: "symbol",
      source: sourceId(kind),
      filter: ["has", "heading"],
      layout: {
        "icon-image": arrow,
        "icon-rotate": ["get", "heading"],
        "icon-rotation-alignment": "map",
        "icon-allow-overlap": true,
        "icon-ignore-placement": true,
      },
    });
  }

  // Labels last so no kind's dots can cover another kind's text.
  for (const kind of MAP_KINDS) {
    map.addLayer({
      id: layerId(kind, "label"),
      type: "symbol",
      source: sourceId(kind),
      layout: {
        "text-field": ["get", "label"],
        "text-font": ["Noto Sans Regular"],
        "text-size": 11,
        "text-anchor": "top",
        "text-offset": [0, 0.7],
        // Dropping a colliding label beats hiding the target it belongs to.
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

function highlight(map: MapLibreMap | null, selected: { kind: MapKind; id: string } | null): void {
  if (map === null || !map.getLayer(layerId(MAP_KINDS[0], "dot"))) {
    return;
  }
  for (const kind of MAP_KINDS) {
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
function frame(map: MapLibreMap, collection: TargetCollection): void {
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

const ARROW_SCALE = 2;
const ARROW_PX = 18;

/** A course indicator pointing north at rotation 0, drawn clear of the 4 px dot so the two read
 * as one symbol. `null` when the browser gives us no 2D context — the map then draws dots only. */
function arrowImage(color: string, edge: string): ImageData | null {
  const canvas = document.createElement("canvas");
  canvas.width = ARROW_PX * ARROW_SCALE;
  canvas.height = ARROW_PX * ARROW_SCALE;
  const ctx = canvas.getContext("2d");
  if (ctx === null) {
    return null;
  }
  ctx.scale(ARROW_SCALE, ARROW_SCALE);
  const mid = ARROW_PX / 2;
  ctx.beginPath();
  ctx.moveTo(mid, 1.5);
  ctx.lineTo(mid + 3.5, 8);
  ctx.lineTo(mid, 6.5);
  ctx.lineTo(mid - 3.5, 8);
  ctx.closePath();
  ctx.strokeStyle = edge;
  ctx.lineWidth = 1;
  ctx.lineJoin = "round";
  ctx.stroke();
  ctx.fillStyle = color;
  ctx.fill();
  return ctx.getImageData(0, 0, canvas.width, canvas.height);
}

function findDetail(kind: MapKind, id: string): TargetDetail | null {
  const station = useDecodedStore.getState().stations[kind]?.find((row) => row.id === id);
  return station === undefined ? null : targetDetail(station);
}

function sameCounts(a: Counts, b: Counts): boolean {
  return MAP_KINDS.every((kind) => a[kind] === b[kind]);
}

/** Reads a theme token off the live element so the map follows the app's palette rather than
 * pinning a second copy of it. */
function themeColor(element: Element, token: string, fallback: string): string {
  const value = getComputedStyle(element).getPropertyValue(token).trim();
  return value === "" ? fallback : value;
}

/** Absolute UTC, not "12 s ago": a wall clock does not need a re-render to stay true. */
function formatUtc(ms: number): string {
  return `${new Date(ms).toISOString().slice(11, 19)}Z`;
}
