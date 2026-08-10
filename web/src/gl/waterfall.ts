// WebGL2 scrolling waterfall (PLAN §9, §10). Each spectrum frame is one row in an R8 texture
// ring; a full-screen quad samples it through the view window and maps intensity through a
// colormap in the fragment shader.
//
// Every colormap here is perceptually uniform and monotone in luminance (DESIGN.md §2): jet and
// its relatives invent bands in smooth data, so they are not offered.

const HISTORY_ROWS = 1024;

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
  // Newest row at the top; older rows scroll downward, one history row per device pixel, so
  // no arriving row is ever skipped and the scroll rate is the frame rate. uRows is clamped
  // to the ring size by the caller, so the bottom edge can never wrap back onto the newest row.
  float rowsBack = (1.0 - vUv.y) * (uRows - 1.0);
  float row = mod(uWrite - 1.0 - rowsBack, uHeight);
  float ty = (row + 0.5) / uHeight;
  float tx = uViewStart + vUv.x * uViewWidth;
  float v = texture(uTex, vec2(tx, ty)).r;
  fragColor = vec4(colormap(v), 1.0);
}`;

export class WaterfallRenderer {
  private readonly gl: WebGL2RenderingContext;
  private readonly canvas: HTMLCanvasElement;
  private readonly program: WebGLProgram;
  private readonly texture: WebGLTexture;
  private readonly uWrite: WebGLUniformLocation | null;
  private readonly uHeight: WebGLUniformLocation | null;
  private readonly uRows: WebGLUniformLocation | null;
  private readonly uViewStart: WebGLUniformLocation | null;
  private readonly uViewWidth: WebGLUniformLocation | null;
  private readonly uMap: WebGLUniformLocation | null;
  private readonly vao: WebGLVertexArrayObject | null;
  private readonly quadBuffer: WebGLBuffer | null;
  private width = 0;
  private writeRow = 0;
  private raf = 0;
  private viewStart = 0;
  private viewWidth = 1;
  private map = 0;

  constructor(canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", { antialias: false, depth: false });
    if (!gl) {
      throw new Error("WebGL2 is required for the waterfall display");
    }
    this.gl = gl;
    this.canvas = canvas;
    this.program = createProgram(gl, VERT, FRAG);
    this.uWrite = gl.getUniformLocation(this.program, "uWrite");
    this.uHeight = gl.getUniformLocation(this.program, "uHeight");
    this.uRows = gl.getUniformLocation(this.program, "uRows");
    this.uViewStart = gl.getUniformLocation(this.program, "uViewStart");
    this.uViewWidth = gl.getUniformLocation(this.program, "uViewWidth");
    this.uMap = gl.getUniformLocation(this.program, "uMap");
    const quad = createFullScreenQuad(gl, this.program);
    this.vao = quad.vao;
    this.quadBuffer = quad.buffer;

    const texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    this.texture = texture;
    this.allocate(1024);

    const loop = () => {
      this.render();
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  /** Append one spectrum row (bin bytes over [dbMin, dbMax]). */
  pushRow(bins: Uint8Array): void {
    const gl = this.gl;
    if (bins.length !== this.width) {
      this.allocate(bins.length);
    }
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      0,
      this.writeRow,
      this.width,
      1,
      gl.RED,
      gl.UNSIGNED_BYTE,
      bins,
    );
    this.writeRow = (this.writeRow + 1) % HISTORY_ROWS;
  }

  /** The visible window over the device span. Applied to the whole history at once, so zooming
   * re-frames what has already been received rather than only what arrives next. */
  setView(start: number, width: number): void {
    this.viewStart = start;
    this.viewWidth = width;
  }

  setColormap(name: Colormap): void {
    this.map = COLORMAPS.indexOf(name);
  }

  dispose(): void {
    cancelAnimationFrame(this.raf);
    this.gl.deleteTexture(this.texture);
    this.gl.deleteProgram(this.program);
    this.gl.deleteVertexArray(this.vao);
    this.gl.deleteBuffer(this.quadBuffer);
    // Deleting the objects does not free the *context*, and browsers cap how many a document
    // may hold (~16). A dock creates and destroys panels for the life of the session, so
    // without this the oldest context is dropped by the browser and some other canvas — not
    // this one — goes black.
    //
    // Deferred and conditional, because losing a context poisons the *canvas*: `getContext`
    // hands back the same dead object forever, and every later shader compile fails with an
    // empty log. StrictMode runs mount→unmount→mount on the very same canvas, so releasing
    // eagerly would kill the renderer it is about to rebuild. By the next macrotask a genuinely
    // removed panel has been detached from the document; a remount has not.
    window.setTimeout(() => {
      if (!this.canvas.isConnected) {
        this.gl.getExtension("WEBGL_lose_context")?.loseContext();
      }
    }, 0);
  }

  private allocate(width: number): void {
    const gl = this.gl;
    this.width = Math.max(1, width);
    this.writeRow = 0;
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.R8,
      this.width,
      HISTORY_ROWS,
      0,
      gl.RED,
      gl.UNSIGNED_BYTE,
      null,
    );
  }

  private render(): void {
    const gl = this.gl;
    this.resizeToDisplay();
    // A panel in a background tab, or one dragged to zero width, measures 0×0. Drawing into it
    // is wasted GPU work every frame, and the rows keep accumulating either way. The *display*
    // size is what goes to zero — `resizeToDisplay` deliberately leaves the backing store at its
    // last non-zero size, so testing `canvas.width` would never fire.
    if (this.canvas.clientWidth === 0 || this.canvas.clientHeight === 0) {
      return;
    }
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.uniform1f(this.uWrite, this.writeRow);
    gl.uniform1f(this.uHeight, HISTORY_ROWS);
    // One row per *CSS* pixel: on a 2× display, counting backing-store rows would halve the
    // scroll speed and double how long the history takes to reach the bottom of the panel.
    const rows = this.canvas.height / Math.min(window.devicePixelRatio || 1, 2);
    gl.uniform1f(this.uRows, Math.max(2, Math.min(HISTORY_ROWS, rows)));
    gl.uniform1f(this.uViewStart, this.viewStart);
    gl.uniform1f(this.uViewWidth, this.viewWidth);
    gl.uniform1i(this.uMap, this.map);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
  }

  private resizeToDisplay(): void {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.round(this.canvas.clientWidth * dpr);
    const h = Math.round(this.canvas.clientHeight * dpr);
    if (w > 0 && h > 0 && (this.canvas.width !== w || this.canvas.height !== h)) {
      this.canvas.width = w;
      this.canvas.height = h;
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
