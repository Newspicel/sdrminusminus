import {
  AttributionControl,
  Map as MapLibreMap,
  NavigationControl,
  setWorkerUrl,
} from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { type RefObject, useEffect, useRef, useState } from "react";
import { describeError, recordEvent } from "../../lib/diagnostics";
import { type BasemapKind, chooseBasemap, fetchOnlineStyle } from "../../lib/map/basemap";
import { installDfLayers } from "../../lib/map/df";
import { installHeatLayer } from "../../lib/map/heat";
import { type MapKind, mapKindsOf } from "../../lib/map/layers";
import { installPositionLayers } from "../../lib/map/position";
import { installPropagationLayers, type PropagationLayer } from "../../lib/map/propagation";
import { installReferenceLayer } from "../../lib/map/reference";
import { installSignalLayers } from "../../lib/map/signal";
import { highlight, hitTarget, installTargetLayers, type Selection } from "../../lib/map/targets";
import { type MapCore, type MapInputs, type MapSinks, readyCore } from "./mapState";

setWorkerUrl(workerUrl);

class CollapsedAttributionControl extends AttributionControl {
  override onAdd(map: MapLibreMap): HTMLElement {
    const container = super.onAdd(map);
    container.classList.add("maplibregl-compact");
    container.classList.remove("maplibregl-compact-show");
    return container;
  }
}

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

function installAll(core: MapCore, inputs: MapInputs, sinks: MapSinks): void {
  const { map, accent, edge } = core;
  installReferenceLayer(map, accent, edge, inputs.references);
  sinks.setHeadings(installTargetLayers(map, edge, inputs.kinds));
  installSignalLayers(map, edge, inputs.signalSamples !== null);
  installPropagationLayers(map, edge, accent, inputs.propagation?.layer ?? null);
  installPositionLayers(map, accent, edge, inputs.positionNodes.length > 0);
  installDfLayers(map, accent, inputs.df !== null);
  installHeatLayer(map, inputs.heatEnabled);
  core.ready = true;
  core.generation += 1;
  highlight(map, inputs.kinds, core.selected);
}

function releaseFraming(core: MapCore): void {
  core.framing.targets.current = true;
  core.framing.position.current = true;
  core.framing.signal.current = true;
}

interface Built {
  basemap: BasemapKind;
  core: MapCore;
}

async function buildCore(
  element: HTMLDivElement,
  inputs: () => MapInputs,
  sinks: MapSinks,
  select: (hit: Selection | null) => void,
  alive: () => boolean,
): Promise<Built | null> {
  const edge = themeColor(element, "--color-bg", "#101113");
  const accent = themeColor(element, "--color-accent", "#76acfc");
  const online = await fetchOnlineStyle();
  if (!alive()) {
    return null;
  }
  const chosen = chooseBasemap(online, edge);
  const map = new MapLibreMap({
    container: element,
    style: chosen.style,
    center: [0, 25],
    zoom: 1,
    attributionControl: false,
  });
  const core: MapCore = {
    map,
    ready: false,
    generation: 0,
    edge,
    accent,
    framing: {
      targets: { current: false },
      position: { current: false },
      signal: { current: false },
    },
    selected: null,
  };
  map.addControl(new CollapsedAttributionControl({ compact: true }));
  map.addControl(new NavigationControl({ showCompass: false }), "top-right");
  map.on("style.load", () => installAll(core, inputs(), sinks));
  map.on("movestart", (event) => {
    if (event.originalEvent !== undefined) {
      releaseFraming(core);
    }
  });
  map.on("click", (event) => {
    const hit = hitTarget(map, event);
    core.selected = hit;
    highlight(map, inputs().kinds, hit);
    select(hit);
  });
  map.on("mousemove", (event) => {
    map.getCanvas().style.cursor = hitTarget(map, event) === null ? "" : "pointer";
  });
  return { basemap: chosen.kind, core };
}

export function useMapInstance(
  containerRef: RefObject<HTMLDivElement | null>,
  inputsRef: RefObject<MapInputs>,
  sinks: MapSinks,
  onSelect: (hit: Selection | null) => void,
): { coreRef: RefObject<MapCore | null>; basemap: BasemapKind } {
  const coreRef = useRef<MapCore | null>(null);
  const selectRef = useRef(onSelect);
  const [basemap, setBasemap] = useState<BasemapKind>("pending");
  useEffect(() => {
    selectRef.current = onSelect;
  });
  useEffect(() => {
    const element = containerRef.current;
    if (element === null) {
      return;
    }
    let alive = true;
    let built: MapCore | null = null;
    buildCore(
      element,
      () => inputsRef.current,
      sinks,
      (hit) => selectRef.current(hit),
      () => alive,
    )
      .then((result) => {
        if (result === null) {
          return;
        }
        if (!alive) {
          result.core.map.remove();
          return;
        }
        built = result.core;
        coreRef.current = result.core;
        setBasemap(result.basemap);
      })
      .catch((error: unknown) => recordEvent("error", "map", `map: ${describeError(error)}`));
    return () => {
      alive = false;
      built?.map.remove();
      coreRef.current = null;
    };
  }, [containerRef, inputsRef, sinks]);
  return { coreRef, basemap };
}

export interface LayerKeys {
  kinds: readonly MapKind[];
  references: readonly (readonly [number, number])[];
  positionNodes: readonly string[];
  signalEnabled: boolean;
  propagationLayer: PropagationLayer | null;
  dfEnabled: boolean;
  heatEnabled: boolean;
}

export function useLayerSync(
  coreRef: RefObject<MapCore | null>,
  keys: LayerKeys,
  sinks: MapSinks,
): void {
  const kindsKey = keys.kinds.join(" ");
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core === null) {
      return;
    }
    const wired = mapKindsOf(kindsKey.split(" "));
    sinks.setHeadings(installTargetLayers(core.map, core.edge, wired));
    core.generation += 1;
    if (core.selected !== null && !wired.includes(core.selected.kind)) {
      core.selected = null;
      sinks.setDetail(() => null);
    }
    highlight(core.map, wired, core.selected);
  }, [coreRef, kindsKey, sinks]);

  const referencesKey = JSON.stringify(keys.references);
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core !== null) {
      const positions = JSON.parse(referencesKey) as [number, number][];
      installReferenceLayer(core.map, core.accent, core.edge, positions);
    }
  }, [coreRef, referencesKey]);

  const positionsOn = keys.positionNodes.length > 0;
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core !== null) {
      installPositionLayers(core.map, core.accent, core.edge, positionsOn);
      core.generation += 1;
    }
  }, [coreRef, positionsOn]);

  useLayerToggles(coreRef, keys);
}

function useLayerToggles(coreRef: RefObject<MapCore | null>, keys: LayerKeys): void {
  const { signalEnabled, propagationLayer, dfEnabled, heatEnabled } = keys;
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core !== null) {
      installSignalLayers(core.map, core.edge, signalEnabled);
      core.generation += 1;
    }
  }, [coreRef, signalEnabled]);
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core !== null) {
      installPropagationLayers(core.map, core.edge, core.accent, propagationLayer);
      core.generation += 1;
    }
  }, [coreRef, propagationLayer]);
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core !== null) {
      installDfLayers(core.map, core.accent, dfEnabled);
      core.generation += 1;
    }
  }, [coreRef, dfEnabled]);
  useEffect(() => {
    const core = readyCore(coreRef.current);
    if (core !== null) {
      installHeatLayer(core.map, heatEnabled);
      core.generation += 1;
    }
  }, [coreRef, heatEnabled]);
}

export function useMapActive(
  containerRef: RefObject<HTMLDivElement | null>,
  coreRef: RefObject<MapCore | null>,
  active: boolean,
  basemap: BasemapKind,
): void {
  useEffect(() => {
    const element = containerRef.current;
    if (element === null || !active) {
      return;
    }
    const keepWheel = (event: WheelEvent) => event.stopPropagation();
    element.addEventListener("wheel", keepWheel);
    return () => element.removeEventListener("wheel", keepWheel);
  }, [containerRef, active]);

  useEffect(() => {
    const map = coreRef.current?.map;
    if (map === undefined || basemap === "pending") {
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
  }, [coreRef, active, basemap]);
}
