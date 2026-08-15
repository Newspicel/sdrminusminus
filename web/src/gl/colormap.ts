export const COLORMAPS = ["classic", "magma", "inferno", "plasma", "viridis", "gray"] as const;
export type Colormap = (typeof COLORMAPS)[number];

export const DEFAULT_COLORMAP: Colormap = "classic";

export type Rgb = readonly [number, number, number];

const CLASSIC_STOPS: readonly Rgb[] = [
  [0.0, 0.0, 0.12549],
  [0.0, 0.0, 0.18824],
  [0.0, 0.0, 0.31373],
  [0.0, 0.0, 0.56863],
  [0.11765, 0.56471, 1.0],
  [1.0, 1.0, 1.0],
  [1.0, 1.0, 0.0],
  [0.99608, 0.42745, 0.08627],
  [0.99608, 0.42745, 0.08627],
  [1.0, 0.0, 0.0],
  [1.0, 0.0, 0.0],
  [0.77647, 0.0, 0.0],
  [0.62353, 0.0, 0.0],
  [0.45882, 0.0, 0.0],
  [0.2902, 0.0, 0.0],
];

const POLYNOMIALS: Readonly<Partial<Record<Colormap, readonly Rgb[]>>> = {
  magma: [
    [-0.00213649, -0.00074966, -0.00538613],
    [0.25166054, 0.67752324, 2.4940266],
    [8.35371728, -3.57771951, 0.3144679],
    [-27.66873309, 14.26473078, -13.64921319],
    [52.17613981, -27.94360607, 12.94416944],
    [-50.76852536, 29.04658282, 4.23415299],
    [18.65570507, -11.48977352, -5.60196151],
  ],
  inferno: [
    [0.00021894, 0.001651, -0.0194809],
    [0.10651342, 0.56395644, 3.93271239],
    [11.60249308, -3.97285397, -15.94239411],
    [-41.70399613, 17.43639888, 44.3541452],
    [77.1629357, -33.40235894, -81.80730926],
    [-71.31942824, 32.62606426, 73.20951986],
    [25.13112622, -12.24266895, -23.070325],
  ],
  plasma: [
    [0.05873234, 0.02333671, 0.54334018],
    [2.17651463, 0.23838342, 0.75396046],
    [-2.68946048, -7.45585114, 3.11079994],
    [6.13034835, 42.34618815, -28.51885465],
    [-11.10743619, -82.66631109, 60.13984767],
    [10.02306558, 71.4136177, -54.07218656],
    [-3.65871384, -22.93153465, 18.19190779],
  ],
  viridis: [
    [0.27772733, 0.00540734, 0.33409981],
    [0.10509304, 1.40461353, 1.38459016],
    [-0.33086183, 0.21484756, 0.09509516],
    [-4.6342305, -5.79910097, -19.33244096],
    [6.22826994, 14.17993337, 5.6690552e1],
    [4.775385, -13.74514538, -65.35303263],
    [-5.43545586, 4.64585261, 26.31243525],
  ],
};

export function sampleColormap(map: Colormap, t: number): Rgb {
  const x = Math.min(1, Math.max(0, Number.isFinite(t) ? t : 0));
  if (map === "gray") {
    return [x, x, x];
  }
  const poly = POLYNOMIALS[map];
  if (poly === undefined) {
    return sampleClassic(x);
  }
  const out: [number, number, number] = [0, 0, 0];
  for (let c = 0; c < 3; c++) {
    let acc = 0;
    for (let i = poly.length - 1; i >= 0; i--) {
      acc = (poly[i]?.[c] ?? 0) + x * acc;
    }
    out[c] = Math.min(1, Math.max(0, acc));
  }
  return out;
}

function sampleClassic(t: number): Rgb {
  const x = t * (CLASSIC_STOPS.length - 1);
  const i = Math.min(Math.floor(x), CLASSIC_STOPS.length - 2);
  const lo = CLASSIC_STOPS[i] ?? [0, 0, 0];
  const hi = CLASSIC_STOPS[i + 1] ?? lo;
  const f = x - i;
  return [lo[0] + (hi[0] - lo[0]) * f, lo[1] + (hi[1] - lo[1]) * f, lo[2] + (hi[2] - lo[2]) * f];
}

function glslVec3([r, g, b]: Rgb): string {
  return `vec3(${r.toFixed(8)}, ${g.toFixed(8)}, ${b.toFixed(8)})`;
}

function glslPoly(map: Colormap, index: number): string {
  const poly = POLYNOMIALS[map];
  if (poly === undefined) {
    return "";
  }
  return `  if (uMap == ${index}) { return poly(t, ${poly.map(glslVec3).join(", ")}); }`;
}

export const COLORMAP_GLSL = `
const vec3 CLASSIC[${CLASSIC_STOPS.length}] = vec3[${CLASSIC_STOPS.length}](
${CLASSIC_STOPS.map((stop) => `  ${glslVec3(stop)}`).join(",\n")}
);

vec3 poly(float t, vec3 c0, vec3 c1, vec3 c2, vec3 c3, vec3 c4, vec3 c5, vec3 c6) {
  return clamp(c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6))))), 0.0, 1.0);
}

vec3 classic(float t) {
  float x = t * ${(CLASSIC_STOPS.length - 1).toFixed(1)};
  int i = min(int(floor(x)), ${CLASSIC_STOPS.length - 2});
  return mix(CLASSIC[i], CLASSIC[i + 1], x - float(i));
}

vec3 colormap(float t) {
  t = clamp(t, 0.0, 1.0);
${(["magma", "inferno", "plasma", "viridis"] as const).map((name) => glslPoly(name, COLORMAPS.indexOf(name))).join("\n")}
  if (uMap == ${COLORMAPS.indexOf("gray")}) { return vec3(t); }
  return classic(t);
}
`;
