export interface RoseMark {
  deg: number;
  label: string;
}

export type NeedleWeight = "primary" | "secondary" | "mirror";

export interface RoseNeedle {
  deg: number;
  weight: NeedleWeight;
}

export interface RoseWedge {
  deg: number;
  sigmaDeg: number;
}

export const TRUE_MARKS: readonly RoseMark[] = [
  { deg: 0, label: "N" },
  { deg: 90, label: "E" },
  { deg: 180, label: "S" },
  { deg: 270, label: "W" },
];

export const RELATIVE_MARKS: readonly RoseMark[] = [
  { deg: 0, label: "F" },
  { deg: 90, label: "R" },
  { deg: 180, label: "B" },
  { deg: 270, label: "L" },
];

const NEEDLE_CLASS: Record<NeedleWeight, string> = {
  primary: "stroke-accent stroke-2",
  secondary: "stroke-accent/60",
  mirror: "stroke-ink-faint",
};

const NEEDLE_DASH: Record<NeedleWeight, string | undefined> = {
  primary: undefined,
  secondary: "3 2",
  mirror: "1 2",
};

const MAX_WEDGE_DEG = 179.9;
const MARK_GAP_PX = 10;
const TICK_PX = 6;

export function polarPoint(
  bearingDeg: number,
  radius: number,
  centre: number,
): { x: number; y: number } {
  const angle = ((bearingDeg - 90) * Math.PI) / 180;
  return { x: centre + radius * Math.cos(angle), y: centre + radius * Math.sin(angle) };
}

function point(bearingDeg: number, radius: number, centre: number): string {
  const { x, y } = polarPoint(bearingDeg, radius, centre);
  return `${x.toFixed(2)} ${y.toFixed(2)}`;
}

export function spectrumPath(
  spectrum: readonly number[],
  centre: number,
  inner: number,
  outer: number,
  rotateDeg = 0,
): string {
  const points = spectrum.length;
  if (points === 0) {
    return "";
  }
  const parts = spectrum.map((value, index) => {
    const radius = inner + ((outer - inner) * Math.min(255, Math.max(0, value))) / 255;
    return `${index === 0 ? "M" : "L"}${point((index * 360) / points + rotateDeg, radius, centre)}`;
  });
  return `${parts.join(" ")} Z`;
}

export function sigmaWedge(deg: number, sigmaDeg: number, centre: number, radius: number): string {
  const half = Math.min(MAX_WEDGE_DEG, Math.max(0, sigmaDeg));
  const large = half > 90 ? 1 : 0;
  return [
    `M${centre.toFixed(2)} ${centre.toFixed(2)}`,
    `L${point(deg - half, radius, centre)}`,
    `A${radius} ${radius} 0 ${large} 1 ${point(deg + half, radius, centre)}`,
    "Z",
  ].join(" ");
}

export function Rose({
  label,
  marks,
  spectrum = [],
  rotateDeg = 0,
  needles,
  wedge,
  tickDeg,
  dim = false,
  size = 220,
}: {
  label: string;
  marks: readonly RoseMark[];
  spectrum?: readonly number[];
  rotateDeg?: number;
  needles: readonly RoseNeedle[];
  wedge?: RoseWedge | null;
  tickDeg?: number | null;
  dim?: boolean;
  size?: number;
}) {
  const centre = size / 2;
  const outer = centre - 18;
  const inner = size * 0.12;
  return (
    <svg
      viewBox={`0 0 ${size} ${size}`}
      width={size}
      height={size}
      role="img"
      aria-label={label}
      className={`max-w-full shrink-0 ${dim ? "opacity-45 grayscale" : ""}`}
    >
      <title>{label}</title>
      <circle cx={centre} cy={centre} r={outer} className="fill-none stroke-line" />
      <circle
        cx={centre}
        cy={centre}
        r={(outer + inner) / 2}
        className="fill-none stroke-line/50"
      />
      {marks.map((mark) => (
        <RoseMarkTick key={mark.label} mark={mark} centre={centre} outer={outer} />
      ))}
      {wedge != null && (
        <path d={sigmaWedge(wedge.deg, wedge.sigmaDeg, centre, outer)} className="fill-accent/20" />
      )}
      {spectrum.length > 0 && (
        <path
          d={spectrumPath(spectrum, centre, inner, outer, rotateDeg)}
          className="fill-accent/15 stroke-accent/80"
        />
      )}
      {distinctNeedles(needles).map(([key, needle]) => {
        const tip = polarPoint(needle.deg, outer, centre);
        return (
          <line
            key={key}
            x1={centre}
            y1={centre}
            x2={tip.x}
            y2={tip.y}
            strokeDasharray={NEEDLE_DASH[needle.weight]}
            className={NEEDLE_CLASS[needle.weight]}
          />
        );
      })}
      {tickDeg != null && <HeadingTick deg={tickDeg} centre={centre} outer={outer} />}
      <circle cx={centre} cy={centre} r={2} className="fill-accent" />
    </svg>
  );
}

export function distinctNeedles(needles: readonly RoseNeedle[]): [string, RoseNeedle][] {
  const keyed = new Map<string, RoseNeedle>();
  for (const needle of needles) {
    keyed.set(`${needle.weight}:${needle.deg.toFixed(2)}`, needle);
  }
  return [...keyed];
}

function RoseMarkTick({ mark, centre, outer }: { mark: RoseMark; centre: number; outer: number }) {
  const text = polarPoint(mark.deg, outer + MARK_GAP_PX, centre);
  const tick = polarPoint(mark.deg, outer, centre);
  const root = polarPoint(mark.deg, outer - TICK_PX, centre);
  return (
    <g>
      <line x1={root.x} y1={root.y} x2={tick.x} y2={tick.y} className="stroke-line" />
      <text
        x={text.x}
        y={text.y}
        className="fill-ink-dim font-mono text-[9px]"
        textAnchor="middle"
        dominantBaseline="middle"
      >
        {mark.label}
      </text>
    </g>
  );
}

function HeadingTick({ deg, centre, outer }: { deg: number; centre: number; outer: number }) {
  const tip = polarPoint(deg, outer - TICK_PX, centre);
  const left = polarPoint(deg - 4, outer + 2, centre);
  const right = polarPoint(deg + 4, outer + 2, centre);
  return (
    <path
      d={`M${tip.x.toFixed(2)} ${tip.y.toFixed(2)} L${left.x.toFixed(2)} ${left.y.toFixed(2)} L${right.x.toFixed(2)} ${right.y.toFixed(2)} Z`}
      className="fill-port-array"
    >
      <title>Array forward</title>
    </path>
  );
}
