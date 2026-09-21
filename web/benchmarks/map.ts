import { Map, setWorkerUrl } from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { stats } from "./gpu";

export async function benchmarkMap() {
  setWorkerUrl(workerUrl);
  const container = document.createElement("div");
  container.style.cssText = "width:1280px;height:720px";
  document.body.append(container);
  const map = new Map({
    container,
    center: [0, 25],
    zoom: 2,
    attributionControl: false,
    style: {
      version: 8,
      sources: {
        targets: {
          type: "geojson",
          data: {
            type: "FeatureCollection",
            features: Array.from({ length: 10000 }, (_, i) => ({
              type: "Feature",
              properties: {},
              geometry: {
                type: "Point",
                coordinates: [((i * 137.5) % 360) - 180, ((i * 13.7) % 140) - 70],
              },
            })),
          },
        },
      },
      layers: [
        { id: "background", type: "background", paint: { "background-color": "#14202a" } },
        {
          id: "targets",
          type: "circle",
          source: "targets",
          paint: { "circle-radius": 4, "circle-color": "#4fffb0" },
        },
      ],
    },
  });
  await new Promise<void>((resolve, reject) => {
    map.once("idle", () => resolve());
    map.once("error", (event) => reject(event.error));
  });
  const timings: number[] = [];
  let settled = Promise.resolve();
  for (let i = 0; i < 200; i++) {
    if (i === 199) settled = new Promise<void>((resolve) => map.once("idle", () => resolve()));
    const start = performance.now();
    const rendered = new Promise<void>((resolve) => map.once("render", () => resolve()));
    map.jumpTo({ bearing: i * 0.5, center: [i * 0.1, 25] });
    await rendered;
    if (i >= 20) timings.push(performance.now() - start);
  }
  await settled;
  let idleDraws = 0;
  map.on("render", () => idleDraws++);
  await new Promise((resolve) => setTimeout(resolve, 1000));
  map.remove();
  return { render_interval_ms: stats(timings), idleDraws };
}
