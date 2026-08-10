// WebGL2 scrolling waterfall (PLAN §9, §10). Each spectrum frame is one row in an R8 texture
// ring; a full-screen quad samples it with a per-row scroll offset and maps intensity through a
// magma colormap (perceptual, colorblind-safe — PLAN §10) in the fragment shader.

const HISTORY_ROWS = 1024;

const VERT = `#version 300 es
in vec2 aPos;
out vec2 vUv;
void main() {
  vUv = vec2((aPos.x + 1.0) * 0.5, (aPos.y + 1.0) * 0.5);
  gl_Position = vec4(aPos, 0.0, 1.0);
}`;

// Magma polynomial fit (Matt Zucker / D3-scale-chromatic, public domain).
const FRAG = `#version 300 es
precision highp float;
in vec2 vUv;
out vec4 fragColor;
uniform sampler2D uTex;
uniform float uWrite;
uniform float uHeight;

vec3 magma(float t) {
  const vec3 c0 = vec3(-0.002136, -0.000749, -0.005386);
  const vec3 c1 = vec3(0.251200, 0.677988, 2.494026);
  const vec3 c2 = vec3(8.353717, -3.577719, 0.310912);
  const vec3 c3 = vec3(-27.66873, 14.264730, -13.649213);
  const vec3 c4 = vec3(52.176139, -27.943606, 12.944169);
  const vec3 c5 = vec3(-50.768525, 29.046582, 4.234153);
  const vec3 c6 = vec3(18.655705, -11.489773, -5.601961);
  t = clamp(t, 0.0, 1.0);
  return clamp(c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6))))), 0.0, 1.0);
}

void main() {
  // Newest row at the top; older rows scroll downward. Span the screen over the (uHeight-1)
  // distinct history rows so the bottom edge lands exactly on the oldest row instead of
  // wrapping fractionally back onto the newest one.
  float rowsBack = (1.0 - vUv.y) * (uHeight - 1.0);
  float row = mod(uWrite - 1.0 - rowsBack, uHeight);
  float ty = (row + 0.5) / uHeight;
  float v = texture(uTex, vec2(vUv.x, ty)).r;
  fragColor = vec4(magma(v), 1.0);
}`;

export class WaterfallRenderer {
  private readonly gl: WebGL2RenderingContext;
  private readonly canvas: HTMLCanvasElement;
  private readonly program: WebGLProgram;
  private readonly texture: WebGLTexture;
  private readonly uWrite: WebGLUniformLocation | null;
  private readonly uHeight: WebGLUniformLocation | null;
  private readonly vao: WebGLVertexArrayObject | null;
  private readonly quadBuffer: WebGLBuffer | null;
  private width = 0;
  private writeRow = 0;
  private raf = 0;

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
