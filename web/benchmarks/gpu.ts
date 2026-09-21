import { sampleColormap } from "../src/gl/colormap";
import { attachWaterfall } from "../src/gl/waterfall";

interface Options {
  count: number;
  width: number;
  height: number;
  bins: number;
  renderer: "webgl" | "canvas";
}

export function stats(values: number[]) {
  values.sort((a, b) => a - b);
  return {
    median: values[Math.floor(values.length / 2)],
    p95: values[Math.floor(values.length * 0.95)],
  };
}

function canvasView(canvas: HTMLCanvasElement, bins: number, height: number) {
  const context = canvas.getContext("2d");
  const history = document.createElement("canvas");
  history.width = bins;
  history.height = 1024;
  const cache = history.getContext("2d");
  if (!context || !cache) throw new Error("Canvas 2D unavailable");
  const row = cache.createImageData(bins, 1);
  const colors = Array.from({ length: 256 }, (_, i) =>
    sampleColormap("classic", i / 255).map((v) => Math.round(v * 255)),
  );
  let write = 0;
  const pushRow = (input: Uint8Array) => {
    for (let i = 0; i < bins; i++) {
      const color = colors[input[i] ?? 0];
      row.data[i * 4] = color?.[0] ?? 0;
      row.data[i * 4 + 1] = color?.[1] ?? 0;
      row.data[i * 4 + 2] = color?.[2] ?? 0;
      row.data[i * 4 + 3] = 255;
    }
    cache.putImageData(row, 0, 1023 - write);
    write = (write + 1) % 1024;
  };
  return {
    pushRow,
    paint() {
      const top = (1024 - write) % 1024;
      const first = Math.min(height, 1024 - top);
      context.drawImage(history, 0, top, bins, first, 0, 0, canvas.width, first * 2);
      if (first < height)
        context.drawImage(
          history,
          0,
          0,
          bins,
          height - first,
          0,
          first * 2,
          canvas.width,
          (height - first) * 2,
        );
    },
    dispose() {},
  };
}

export async function waterfall(options: Options) {
  const { count, width, height, bins, renderer } = options;
  const nativeRaf = window.requestAnimationFrame.bind(window);
  const nextFrame = () => new Promise<number>((resolve) => nativeRaf(resolve));
  const probe = document.createElement("canvas").getContext("webgl2");
  if (!probe) throw new Error("WebGL2 unavailable");
  const debug = probe.getExtension("WEBGL_debug_renderer_info");
  const adapter = debug ? String(probe.getParameter(debug.UNMASKED_RENDERER_WEBGL)) : "unknown";
  probe.getExtension("WEBGL_lose_context")?.loseContext();
  let measuring = false;
  let draws = 0;
  const submission: number[] = [];
  const queries: { gl: WebGL2RenderingContext; query: WebGLQuery }[] = [];
  const extensions = new Map<
    WebGL2RenderingContext,
    { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number } | null
  >();
  const originalDraw = Object.getOwnPropertyDescriptor(
    WebGL2RenderingContext.prototype,
    "drawArrays",
  )?.value as WebGL2RenderingContext["drawArrays"];
  WebGL2RenderingContext.prototype.drawArrays = function (...args) {
    if (!extensions.has(this))
      extensions.set(this, this.getExtension("EXT_disjoint_timer_query_webgl2"));
    const extension = extensions.get(this);
    const query = measuring && extension ? this.createQuery() : null;
    if (query && extension) this.beginQuery(extension.TIME_ELAPSED_EXT, query);
    if (measuring) draws++;
    originalDraw.apply(this, args);
    if (query && extension) {
      this.endQuery(extension.TIME_ELAPSED_EXT);
      queries.push({ gl: this, query });
    }
  };
  window.requestAnimationFrame = (callback) =>
    nativeRaf((time) => {
      const start = performance.now();
      callback(time);
      if (measuring) submission.push(performance.now() - start);
    });
  const views = Array.from({ length: count }, () => {
    const canvas = document.createElement("canvas");
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    canvas.width = width * 2;
    canvas.height = height * 2;
    document.body.append(canvas);
    return renderer === "webgl" ? attachWaterfall(canvas) : canvasView(canvas, bins, height);
  });
  const row = Uint8Array.from({ length: bins }, (_, i) => (i * 13 + (i >> 4)) % 256);
  for (let i = 0; i < 1024; i++) for (const view of views) view.pushRow(row);
  for (let i = 0; i < 30; i++) await nextFrame();
  const intervals: number[] = [];
  const updates: number[] = [];
  let previous = await nextFrame();
  let nextUpdate = previous;
  measuring = true;
  for (let frame = 0; frame < 180; frame++) {
    const now = await nextFrame();
    intervals.push(now - previous);
    previous = now;
    if (now >= nextUpdate) {
      nextUpdate += 1000 / 30;
      const start = performance.now();
      row[0] = (row[0] ?? 0) ^ 255;
      for (const view of views) {
        view.pushRow(row);
        if (renderer === "canvas") (view as ReturnType<typeof canvasView>).paint();
      }
      updates.push(performance.now() - start);
    }
  }
  measuring = false;
  const activeDraws = draws;
  const activeSubmission = stats([...submission]);
  for (let i = 0; i < 3; i++) await nextFrame();
  draws = 0;
  measuring = true;
  for (let i = 0; i < 61; i++) await nextFrame();
  measuring = false;
  for (const [gl, extension] of extensions) {
    if (extension && gl.getParameter(extension.GPU_DISJOINT_EXT))
      throw new Error("GPU timer became disjoint");
    if (gl.getError() !== gl.NO_ERROR) throw new Error("WebGL rendering error");
  }
  for (const canvas of document.querySelectorAll("canvas")) {
    const context = canvas.getContext("2d");
    if (context?.getImageData(canvas.width / 2, canvas.height / 2, 1, 1).data[3] !== 255)
      throw new Error("Waterfall did not render");
  }
  const gpuTimes = queries.flatMap(({ gl, query }) => {
    const time = gl.getQueryParameter(query, gl.QUERY_RESULT_AVAILABLE)
      ? [Number(gl.getQueryParameter(query, gl.QUERY_RESULT)) / 1e6]
      : [];
    gl.deleteQuery(query);
    return time;
  });
  for (const view of views) view.dispose();
  window.requestAnimationFrame = nativeRaf;
  WebGL2RenderingContext.prototype.drawArrays = originalDraw;
  return {
    adapter,
    gpu_draw_ms: stats(gpuTimes),
    update_ms: stats(updates),
    submission_ms: activeSubmission,
    frame_ms: stats(intervals),
    activeDraws,
    idleDraws: draws,
  };
}
