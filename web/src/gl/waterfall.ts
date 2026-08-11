// The one WebGL2 context in the document, shared by every scope on the canvas (CANVAS §7,
// PLAN §17). Browsers cap live contexts at roughly 8–16 per document, and a patch full of scope
// nodes walks straight past that: the browser then drops the oldest context and some *other*
// plot goes black. So the context lives here, module-level, on one offscreen canvas. A plot
// owns nothing but its history texture and its window into it; one rAF loop draws each visible
// plot into the shared buffer at that plot's device-pixel size and blits the result into the
// plot's own 2D canvas.
//
// Each spectrum frame is one row of an R8 texture ring (PLAN §9, §10); a full-screen quad
// samples the ring through the view window and maps intensity through a colormap in the
// fragment shader.
//
// Every colormap here is perceptually uniform and monotone in luminance (DESIGN.md §2): jet and
// its relatives invent bands in smooth data, so they are not offered.

import { backingPx, fitExtent, nextRingRow, pixelRatio, rowsForHeight, zoomOf } from "./raster";

const HISTORY_ROWS = 1024;

/** A plot just outside the viewport is drawn anyway, so panning it into view shows history
 * rather than a blank rectangle filling in. */
const PREROLL_MARGIN = "128px";

export const COLORMAPS = ["magma", "inferno", "plasma", "viridis", "gray"] as const;
export type Colormap = (typeof COLORMAPS)[number];

const VERT = `#version 300 es
in vec2 aPos;
out vec2 vUv;
void main() {
  vUv = vec2((aPos.x + 1.0) * 0.5, (aPos.y + 1.0) * 0.5);
  gl_Position = vec4(aPos, 0.0, 1.0);
}`;

// Polynomial fits of the matplotlib colormaps (Matt Zucker, public domain).
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

vec3 poly(float t, vec3 c0, vec3 c1, vec3 c2, vec3 c3, vec3 c4, vec3 c5, vec3 c6) {
  return clamp(c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6))))), 0.0, 1.0);
}

vec3 colormap(float t) {
  t = clamp(t, 0.0, 1.0);
  if (uMap == 1) {
    return poly(t,
      vec3(0.00021894, 0.00165100, -0.01948090),
      vec3(0.10651342, 0.56395644, 3.93271239),
      vec3(11.60249308, -3.97285397, -15.94239411),
      vec3(-41.70399613, 17.43639888, 44.35414520),
      vec3(77.16293570, -33.40235894, -81.80730926),
      vec3(-71.31942824, 32.62606426, 73.20951986),
      vec3(25.13112622, -12.24266895, -23.07032500));
  }
  if (uMap == 2) {
    return poly(t,
      vec3(0.05873234, 0.02333671, 0.54334018),
      vec3(2.17651463, 0.23838342, 0.75396046),
      vec3(-2.68946048, -7.45585114, 3.11079994),
      vec3(6.13034835, 42.34618815, -28.51885465),
      vec3(-11.10743619, -82.66631109, 60.13984767),
      vec3(10.02306558, 71.41361770, -54.07218656),
      vec3(-3.65871384, -22.93153465, 18.19190779));
  }
  if (uMap == 3) {
    return poly(t,
      vec3(0.27772733, 0.00540734, 0.33409981),
      vec3(0.10509304, 1.40461353, 1.38459016),
      vec3(-0.33086183, 0.21484756, 0.09509516),
      vec3(-4.63423050, -5.79910097, -19.33244096),
      vec3(6.22826994, 14.17993337, 56.69055260),
      vec3(4.77638500, -13.74514538, -65.35303263),
      vec3(-5.43545586, 4.64585261, 26.31243525));
  }
  if (uMap == 4) {
    return vec3(t);
  }
  return poly(t,
    vec3(-0.00213649, -0.00074966, -0.00538613),
    vec3(0.25166054, 0.67752324, 2.49402660),
    vec3(8.35371728, -3.57771951, 0.31446790),
    vec3(-27.66873309, 14.26473078, -13.64921319),
    vec3(52.17613981, -27.94360607, 12.94416944),
    vec3(-50.76852536, 29.04658282, 4.23415299),
    vec3(18.65570507, -11.48977352, -5.60196151));
}

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

/**
 * One plot's share of the shared renderer. Created by `attachWaterfall`, which owns everything
 * else: the caller feeds it rows and tells it what to show, and drawing — when, how large, and
 * whether at all — is the renderer's decision.
 */
export interface WaterfallView {
  /** Append one spectrum row (bin bytes over the frame's [dbMin, dbMax]). Rows accumulate while
   * the plot is off screen; only the drawing is skipped. They are dropped only while the context
   * is gone, when there is no history left for them to extend. */
  pushRow(bins: Uint8Array): void;
  /** The visible window over the device span, `start` and `width` as fractions of it. Applied
   * to the whole history at once, so zooming re-frames what has already been received rather
   * than only what arrives next. */
  setWindow(start: number, width: number): void;
  setColormap(name: Colormap): void;
  /** Release this plot's texture, and the context itself when it was the last plot. */
  dispose(): void;
}

/** What the renderer cannot do right now, or `null` once it can again. */
export type WaterfallStatus = (error: string | null) => void;

/** The one failure that clears itself: the browser is expected to hand the context back. */
const CONTEXT_LOST = "the graphics context was lost, waiting for the browser to restore it";

/**
 * Draw a waterfall into `canvas`, a 2D canvas the caller owns and sizes with CSS. Throws when
 * the browser cannot give us WebGL2 at all — the scope face catches that and keeps its trace.
 *
 * Failures that arrive later come through `onStatus` instead: a GPU or driver reset takes the
 * context out from under every plot at once, and there is no call in flight to throw out of.
 */
export function attachWaterfall(
  canvas: HTMLCanvasElement,
  onStatus: WaterfallStatus = () => {},
): WaterfallView {
  // Acquired before the plot exists, so a browser with no WebGL2 at all throws before anything
  // has been allocated to clean up. The exception is a lost context waiting to be restored: a
  // second one would spend another of the browser's few and leave the first to come back to
  // nothing, so the plot starts detached and the render loop attaches it once the first returns.
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
/**
 * The context a reset took, held until the browser restores it. That restore revives *this*
 * `WebGL2RenderingContext` object, so building a replacement meanwhile would both spend another
 * of the browser's few and leave this one to come back to nothing.
 */
let recovering: Shared | null = null;

/** One plot's texture and the context that owns it — nulled together, because a context loss
 * takes both and neither is meaningful without the other. */
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
  // Starts hidden: the observer reports the real answer within a frame or two, and a plot drawn
  // before that would be one the operator cannot see.
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

  /** Take a history texture from `context`. Idempotent, so the render loop can offer the live
   * context to every plot on every frame and only a plot that lost one pays for it. */
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

  /** The texture died with the context. The plot's own canvas is wiped with it: the history is
   * gone either way, and a frozen last frame is a lie about a live radio. */
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
    // Rows arriving while the context is gone are dropped, not queued: they would extend a
    // history that no longer exists.
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
      // A restore has nothing left to come back to; the canvas goes with the last plot.
      recovering = null;
      release();
    }
  }

  /** Bring the plot's own canvas up to its device-pixel size and report it, or `null` when this
   * plot must not be drawn: off screen, or laid out at zero — a node collapsed to a rack
   * placeholder, or one dragged to nothing. */
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
    // GL's origin is bottom-left, so the viewport just drawn is the bottom w×h of the shared
    // buffer. Copying it here, inside the same animation frame, is what makes
    // `preserveDrawingBuffer` unnecessary: the drawing buffer is cleared when the browser
    // composites it, which is after this callback returns.
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
    // No context to draw into, which after a loss means one is being restored. The loop keeps
    // running rather than being cancelled and restarted: an idle rAF costs nothing, and recovery
    // is then a plot re-attaching on the next frame instead of a second lifecycle to get right.
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
    // Nothing visible: the buffer keeps its size, so panning a node back into view costs a draw
    // and not a reallocation.
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
  // A plot arrived, so whatever release was pending is cancelled — see `release` for why the
  // teardown is deferred in the first place.
  window.clearTimeout(teardown);
  teardown = 0;
  // A failure is not remembered: the browser's cap counts *every* context in the document, ours
  // and MapLibre's, so a refusal while a map face is open is one a later scope may not meet.
  shared ??= create();
  return shared;
}

/**
 * Give the context back once no plot is left. Deleting the objects does not free the *context*,
 * and the cap this whole module exists for is on contexts.
 *
 * Deferred, because losing a context poisons the *canvas* it came from: `getContext` hands back
 * the same dead object forever, and every later shader compile fails with an empty log. React
 * StrictMode runs mount→unmount→mount, so releasing eagerly would kill the renderer it is about
 * to rebuild. By the next macrotask the second mount has already called `acquire`, which
 * cancels this; a workspace whose last scope really was removed has not.
 */
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
    // The canvas is dropped with the context, so the next `acquire` builds a fresh pair rather
    // than asking a poisoned canvas for a context it can never give.
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
    // A context whose shader would not compile still counts against the browser's cap, and
    // `acquire` retries on the next face. Handing it back is what keeps a driver that refuses
    // the shader from costing one live context per attempt.
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    throw error;
  }
}

/** Everything a context owns, split out from `create` because a restored context is the same
 * object with every resource it held deleted: only this half is ever built twice. */
function build(canvas: HTMLCanvasElement, gl: WebGL2RenderingContext): Shared {
  const program = createProgram(gl, VERT, FRAG);
  const quad = createFullScreenQuad(gl, program);
  // Rows are single-byte R8 and any bin count is legal, so the default four-byte row alignment
  // would misread every frame whose width is not a multiple of four.
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

/**
 * A GPU or driver reset took the context, and with it every plot's history. `preventDefault` is
 * what asks the browser for it back: without a handler that calls it the browser never tries,
 * and the canvas stays poisoned for good (see `release`) — one reset then blacks out every scope
 * in the workspace until a reload, which is the very failure this module exists to prevent.
 *
 * A deliberate teardown fires this same event. `release` clears `shared` before it calls
 * `loseContext`, so the test below tells the two apart and a teardown is never rebuilt.
 */
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
    // Not retried here: a driver that refuses the shader now refuses it every frame too. The
    // plots stay dark and say why until the next face mounts, whose `acquire` builds a fresh
    // context that `draw` then re-attaches every one of them to.
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
  // Two triangles covering clip space.
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
