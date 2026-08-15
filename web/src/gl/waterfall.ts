import { COLORMAP_GLSL, COLORMAPS, type Colormap } from "./colormap";
import {
  backingPx,
  fitExtent,
  nextRingRow,
  pixelRatio,
  rowsForHeight,
  seedPlacement,
  zoomOf,
} from "./raster";

export { COLORMAPS, type Colormap, DEFAULT_COLORMAP } from "./colormap";

const HISTORY_ROWS = 1024;

const PREROLL_MARGIN = "128px";

const VERT = `#version 300 es
in vec2 aPos;
out vec2 vUv;
void main() {
  vUv = vec2((aPos.x + 1.0) * 0.5, (aPos.y + 1.0) * 0.5);
  gl_Position = vec4(aPos, 0.0, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
in vec2 vUv;
out vec4 fragColor;
uniform sampler2D uTex;
uniform float uWrite;
uniform float uHeight;
uniform float uRows;
uniform float uViewStart;
uniform float uViewWidth;
uniform int uMap;

${COLORMAP_GLSL}

void main() {
  // Newest row at the top; older rows scroll downward, one history row per *layout* pixel — the
  // rule rowsForHeight computes, and where the reason it is not device pixels is written. uRows
  // is clamped to the ring size by the caller, so the bottom edge can never wrap back onto the
  // newest row.
  float rowsBack = (1.0 - vUv.y) * (uRows - 1.0);
  float row = mod(uWrite - 1.0 - rowsBack, uHeight);
  float ty = (row + 0.5) / uHeight;
  float tx = uViewStart + vUv.x * uViewWidth;
  float v = texture(uTex, vec2(tx, ty)).r;
  fragColor = vec4(colormap(v), 1.0);
}`;

export interface WaterfallView {
  pushRow(bins: Uint8Array): void;
  seed(rows: Uint8Array, count: number, bins: number): void;
  setWindow(start: number, width: number): void;
  setColormap(name: Colormap): void;
  dispose(): void;
}

export type WaterfallStatus = (error: string | null) => void;

const CONTEXT_LOST = "the graphics context was lost, waiting for the browser to restore it";

export function attachWaterfall(
  canvas: HTMLCanvasElement,
  onStatus: WaterfallStatus = () => {},
): WaterfallView {
  const context = recovering === null ? acquire() : null;
  const plot = new Plot(canvas, onStatus);
  if (context === null) {
    onStatus(CONTEXT_LOST);
  } else {
    plot.attach(context);
  }
  plots.add(plot);
  if (frame === 0) {
    frame = requestAnimationFrame(draw);
  }
  return plot;
}

interface Uniforms {
  write: WebGLUniformLocation | null;
  height: WebGLUniformLocation | null;
  rows: WebGLUniformLocation | null;
  viewStart: WebGLUniformLocation | null;
  viewWidth: WebGLUniformLocation | null;
  map: WebGLUniformLocation | null;
}

interface Shared {
  canvas: HTMLCanvasElement;
  gl: WebGL2RenderingContext;
  program: WebGLProgram;
  uniforms: Uniforms;
  vao: WebGLVertexArrayObject | null;
  quad: WebGLBuffer | null;
}

let shared: Shared | null = null;
const plots = new Set<Plot>();
let frame = 0;
let teardown = 0;
let recovering: Shared | null = null;

interface Attachment {
  context: Shared;
  texture: WebGLTexture;
}

class Plot implements WaterfallView {
  private readonly ctx: CanvasRenderingContext2D | null;
  private readonly observer: IntersectionObserver;
  private live: Attachment | null = null;
  private bins = 0;
  private writeRow = 0;
  private windowStart = 0;
  private windowWidth = 1;
  private map = 0;
  private ratio = 1;
  private onScreen = false;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly onStatus: WaterfallStatus,
  ) {
    this.ctx = canvas.getContext("2d");
    this.observer = new IntersectionObserver(
      (entries) => {
        this.onScreen = entries[entries.length - 1]?.isIntersecting ?? false;
      },
      { rootMargin: PREROLL_MARGIN },
    );
    this.observer.observe(canvas);
  }

  attach(context: Shared): void {
    if (this.live?.context === context) {
      return;
    }
    const gl = context.gl;
    const texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    this.live = { context, texture };
    this.allocate(1024);
    this.onStatus(null);
  }

  detach(): void {
    this.live = null;
    this.bins = 0;
    this.writeRow = 0;
    this.ctx?.clearRect(0, 0, this.canvas.width, this.canvas.height);
  }

  report(error: string | null): void {
    this.onStatus(error);
  }

  pushRow(bins: Uint8Array): void {
    const live = this.live;
    if (live === null || bins.length === 0) {
      return;
    }
    const gl = live.context.gl;
    if (bins.length !== this.bins) {
      this.allocate(bins.length);
    }
    gl.bindTexture(gl.TEXTURE_2D, live.texture);
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      0,
      this.writeRow,
      this.bins,
      1,
      gl.RED,
      gl.UNSIGNED_BYTE,
      bins,
    );
    this.writeRow = nextRingRow(this.writeRow, HISTORY_ROWS);
  }

  seed(rows: Uint8Array, count: number, bins: number): void {
    const live = this.live;
    if (live === null || bins === 0) {
      return;
    }
    const place = seedPlacement(count, HISTORY_ROWS);
    if (place.rows === 0) {
      return;
    }
    this.allocate(bins);
    const gl = live.context.gl;
    gl.bindTexture(gl.TEXTURE_2D, live.texture);
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      0,
      0,
      bins,
      place.rows,
      gl.RED,
      gl.UNSIGNED_BYTE,
      rows.subarray(place.skip * bins, (place.skip + place.rows) * bins),
    );
    this.writeRow = place.write;
  }

  setWindow(start: number, width: number): void {
    this.windowStart = start;
    this.windowWidth = width;
  }

  setColormap(name: Colormap): void {
    this.map = COLORMAPS.indexOf(name);
  }

  dispose(): void {
    this.observer.disconnect();
    plots.delete(this);
    const live = this.live;
    if (live !== null) {
      live.context.gl.deleteTexture(live.texture);
      this.live = null;
    }
    if (plots.size === 0) {
      cancelAnimationFrame(frame);
      frame = 0;
      recovering = null;
      release();
    }
  }

  measure(): { w: number; h: number } | null {
    if (!this.onScreen) {
      return null;
    }
    const cssWidth = this.canvas.clientWidth;
    const cssHeight = this.canvas.clientHeight;
    if (cssWidth === 0 || cssHeight === 0) {
      return null;
    }
    const rect = this.canvas.getBoundingClientRect();
    this.ratio = pixelRatio(window.devicePixelRatio, zoomOf(rect.width, cssWidth));
    const w = backingPx(cssWidth, this.ratio);
    const h = backingPx(cssHeight, this.ratio);
    if (w === 0 || h === 0) {
      return null;
    }
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    return { w, h };
  }

  paint(w: number, h: number): void {
    const live = this.live;
    if (live === null) {
      return;
    }
    const { gl, canvas: buffer, uniforms } = live.context;
    gl.viewport(0, 0, w, h);
    gl.bindTexture(gl.TEXTURE_2D, live.texture);
    gl.uniform1f(uniforms.write, this.writeRow);
    gl.uniform1f(uniforms.height, HISTORY_ROWS);
    gl.uniform1f(uniforms.rows, rowsForHeight(h, this.ratio, HISTORY_ROWS));
    gl.uniform1f(uniforms.viewStart, this.windowStart);
    gl.uniform1f(uniforms.viewWidth, this.windowWidth);
    gl.uniform1i(uniforms.map, this.map);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    this.ctx?.drawImage(buffer, 0, buffer.height - h, w, h, 0, 0, w, h);
  }

  private allocate(bins: number): void {
    const live = this.live;
    if (live === null) {
      return;
    }
    const gl = live.context.gl;
    this.bins = Math.max(1, bins);
    this.writeRow = 0;
    gl.bindTexture(gl.TEXTURE_2D, live.texture);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.R8,
      this.bins,
      HISTORY_ROWS,
      0,
      gl.RED,
      gl.UNSIGNED_BYTE,
      null,
    );
  }
}

function draw(): void {
  frame = requestAnimationFrame(draw);
  const context = shared;
  if (context === null) {
    return;
  }
  const pending: { plot: Plot; w: number; h: number }[] = [];
  let width = 0;
  let height = 0;
  for (const plot of plots) {
    plot.attach(context);
    const size = plot.measure();
    if (size === null) {
      continue;
    }
    pending.push({ plot, ...size });
    width = Math.max(width, size.w);
    height = Math.max(height, size.h);
  }
  if (pending.length === 0) {
    return;
  }
  const w = fitExtent(context.canvas.width, width);
  const h = fitExtent(context.canvas.height, height);
  if (w !== context.canvas.width || h !== context.canvas.height) {
    context.canvas.width = w;
    context.canvas.height = h;
  }
  context.gl.useProgram(context.program);
  context.gl.bindVertexArray(context.vao);
  for (const entry of pending) {
    entry.plot.paint(entry.w, entry.h);
  }
}

function acquire(): Shared {
  window.clearTimeout(teardown);
  teardown = 0;
  shared ??= create();
  return shared;
}

function release(): void {
  if (teardown !== 0 || shared === null) {
    return;
  }
  teardown = window.setTimeout(() => {
    teardown = 0;
    const context = shared;
    if (context === null || plots.size > 0) {
      return;
    }
    shared = null;
    context.gl.deleteProgram(context.program);
    context.gl.deleteVertexArray(context.vao);
    context.gl.deleteBuffer(context.quad);
    context.gl.getExtension("WEBGL_lose_context")?.loseContext();
  }, 0);
}

function create(): Shared {
  const canvas = document.createElement("canvas");
  const gl = canvas.getContext("webgl2", { antialias: false, depth: false });
  if (!gl) {
    throw new Error("WebGL2 is required for the waterfall display");
  }
  canvas.addEventListener("webglcontextlost", onContextLost);
  canvas.addEventListener("webglcontextrestored", onContextRestored);
  try {
    return build(canvas, gl);
  } catch (error) {
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    throw error;
  }
}

function build(canvas: HTMLCanvasElement, gl: WebGL2RenderingContext): Shared {
  const program = createProgram(gl, VERT, FRAG);
  const quad = createFullScreenQuad(gl, program);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  return {
    canvas,
    gl,
    program,
    vao: quad.vao,
    quad: quad.buffer,
    uniforms: {
      write: gl.getUniformLocation(program, "uWrite"),
      height: gl.getUniformLocation(program, "uHeight"),
      rows: gl.getUniformLocation(program, "uRows"),
      viewStart: gl.getUniformLocation(program, "uViewStart"),
      viewWidth: gl.getUniformLocation(program, "uViewWidth"),
      map: gl.getUniformLocation(program, "uMap"),
    },
  };
}

function onContextLost(event: Event): void {
  const context = shared;
  if (context === null || event.target !== context.canvas) {
    return;
  }
  event.preventDefault();
  shared = null;
  recovering = context;
  for (const plot of plots) {
    plot.detach();
    plot.report(CONTEXT_LOST);
  }
}

function onContextRestored(event: Event): void {
  const context = recovering;
  if (context === null || event.target !== context.canvas) {
    return;
  }
  recovering = null;
  try {
    shared = build(context.canvas, context.gl);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    for (const plot of plots) {
      plot.report(message);
    }
  }
}

function createFullScreenQuad(
  gl: WebGL2RenderingContext,
  program: WebGLProgram,
): { vao: WebGLVertexArrayObject | null; buffer: WebGLBuffer | null } {
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  const verts = new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]);
  gl.bufferData(gl.ARRAY_BUFFER, verts, gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(program, "aPos");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  return { vao, buffer };
}

function createProgram(gl: WebGL2RenderingContext, vert: string, frag: string): WebGLProgram {
  const program = gl.createProgram();
  gl.attachShader(program, compileShader(gl, gl.VERTEX_SHADER, vert));
  gl.attachShader(program, compileShader(gl, gl.FRAGMENT_SHADER, frag));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(program) ?? "unknown link error";
    throw new Error(`waterfall program link failed: ${log}`);
  }
  return program;
}

function compileShader(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) {
    throw new Error("failed to create shader");
  }
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader) ?? "unknown compile error";
    throw new Error(`shader compile failed: ${log}`);
  }
  return shader;
}
