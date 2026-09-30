import { Button } from "../../components/BaseControls";
import { BTN_SM, CHIP_SM, type Options, TABLE_HEAD } from "../../components/controls";
import { formatMhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { polarPoint } from "../../components/Rose";
import { Segmented } from "../../components/Segmented";
import { SettingRow, Settings } from "../../components/Settings";
import type { ArrayElement, ArrayGeometry, GeometryKind, Winding } from "../../lib/types";
import {
  convertGeometry,
  type ElementAxis,
  MAX_EXTENT_M,
  MIN_ELEMENTS,
  movedElement,
  overlappingRows,
  POSITION_STEP_M,
  paddedPositions,
  previewPoints,
  unambiguousHz,
} from "./arrayGeometry";

export const GEOMETRY_OPTIONS: Options<GeometryKind> = [
  { value: "uca", label: "UCA", title: "Circle, lanes in order around it" },
  { value: "ula", label: "ULA", title: "Straight line, even spacing" },
  { value: "explicit", label: "Custom", title: "Your own element positions" },
];

const WINDING_OPTIONS: Options<Winding> = [
  { value: "clockwise", label: "CW", title: "Lanes run clockwise" },
  { value: "counter_clockwise", label: "CCW", title: "Lanes run counter-clockwise" },
];

export const ALIASING = "Aliasing";
export const ALIASING_TITLE = "Elements more than half a wavelength apart";
export const NOT_WIRED = "Not wired";
export const SAME_PLACE = "Same place as another lane";

const PREVIEW_PX = 96;
const MIN_SIZE_M = 0.01;

type Change = (geometry: ArrayGeometry) => void;

export function ArrayGeometryEditor({
  geometry,
  lanes,
  azimuthDeg,
  centerHz,
  onChange,
}: {
  geometry: ArrayGeometry;
  lanes: number;
  azimuthDeg: number | null;
  centerHz: number | null;
  onChange: Change;
}) {
  const limit = unambiguousHz(geometry, lanes);
  const aliasing = limit !== null && centerHz !== null && centerHz > limit;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-start gap-3">
        <Settings className="min-w-0 flex-1">
          <SettingRow label="Shape">
            <Segmented
              label="Geometry"
              value={geometry.kind}
              options={GEOMETRY_OPTIONS}
              onChange={(kind) => onChange(convertGeometry(kind, geometry, lanes))}
            />
          </SettingRow>
          {geometry.kind === "uca" && <CircleRows geometry={geometry} onChange={onChange} />}
          {geometry.kind === "ula" && <LineRows geometry={geometry} onChange={onChange} />}
          <SettingRow label="Max f" title="Highest frequency without aliasing">
            <span className="font-mono text-xs tabular-nums">
              {limit === null ? "-" : formatMhz(limit)}
            </span>
            {aliasing && (
              <span title={ALIASING_TITLE} className={`${CHIP_SM} border-danger/60 text-danger`}>
                {ALIASING}
              </span>
            )}
          </SettingRow>
        </Settings>
        <ArrayPreview geometry={geometry} lanes={lanes} azimuthDeg={azimuthDeg} />
      </div>
      {geometry.kind === "explicit" && (
        <CustomRows positions={geometry.positions} lanes={lanes} onChange={onChange} />
      )}
    </div>
  );
}

function CircleRows({
  geometry,
  onChange,
}: {
  geometry: Extract<ArrayGeometry, { kind: "uca" }>;
  onChange: Change;
}) {
  return (
    <>
      <SettingRow label="Radius">
        <NumberField
          label="Radius"
          unit="m"
          value={geometry.radius_m}
          min={MIN_SIZE_M}
          max={MAX_EXTENT_M}
          step={POSITION_STEP_M}
          onCommit={(radius_m) => onChange({ ...geometry, radius_m })}
        />
      </SettingRow>
      <SettingRow label="First" title="Where lane 1 sits, clockwise from forward">
        <NumberField
          label="First element"
          unit="°"
          value={geometry.first_deg ?? 0}
          min={-360}
          max={360}
          step={0.1}
          onCommit={(first_deg) => onChange({ ...geometry, first_deg })}
        />
      </SettingRow>
      <SettingRow label="Winding">
        <Segmented
          label="Winding"
          value={geometry.winding ?? "clockwise"}
          options={WINDING_OPTIONS}
          onChange={(winding) => onChange({ ...geometry, winding })}
        />
      </SettingRow>
    </>
  );
}

function LineRows({
  geometry,
  onChange,
}: {
  geometry: Extract<ArrayGeometry, { kind: "ula" }>;
  onChange: Change;
}) {
  return (
    <>
      <SettingRow label="Spacing">
        <NumberField
          label="Spacing"
          unit="m"
          value={geometry.spacing_m}
          min={MIN_SIZE_M}
          max={MAX_EXTENT_M}
          step={POSITION_STEP_M}
          onCommit={(spacing_m) => onChange({ ...geometry, spacing_m })}
        />
      </SettingRow>
      <SettingRow label="Axis" title="Lane 1 to last lane, clockwise from forward">
        <NumberField
          label="Axis"
          unit="°"
          value={geometry.axis_deg}
          min={0}
          max={359.9}
          step={0.1}
          onCommit={(axis_deg) => onChange({ ...geometry, axis_deg })}
        />
      </SettingRow>
    </>
  );
}

const AXES: readonly ElementAxis[] = ["x_m", "y_m", "z_m"];

function CustomRows({
  positions,
  lanes,
  onChange,
}: {
  positions: readonly ArrayElement[];
  lanes: number;
  onChange: Change;
}) {
  const overlapping = new Set(overlappingRows(positions, lanes));
  const padded = paddedPositions(positions, lanes);
  const edit = (row: number, axis: ElementAxis, value: number): void =>
    onChange({ kind: "explicit", positions: movedElement(padded, row, axis, value) });
  return (
    <div className="flex flex-col gap-1">
      <table aria-label="Element positions" className="w-full table-fixed">
        <thead>
          <tr>
            <th scope="col" className={`${TABLE_HEAD} w-10`}>
              Lane
            </th>
            {AXES.map((axis) => (
              <th key={axis} scope="col" className={TABLE_HEAD}>
                {axis.charAt(0)} m
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {padded.map((element, row) => (
            <tr
              key={row}
              title={row >= lanes ? NOT_WIRED : overlapping.has(row) ? SAME_PLACE : undefined}
              className={`${row >= lanes ? "opacity-50" : ""} ${overlapping.has(row) ? "text-danger" : ""}`}
            >
              <td className="px-2 font-mono text-xs">{row + 1}</td>
              {AXES.map((axis) => (
                <td key={axis} className="px-0.5 py-0.5">
                  <NumberField
                    label={`Lane ${row + 1} ${axis.charAt(0)}`}
                    className="w-full"
                    value={element[axis] ?? 0}
                    min={-MAX_EXTENT_M}
                    max={MAX_EXTENT_M}
                    step={POSITION_STEP_M}
                    invalid={overlapping.has(row)}
                    onCommit={(value) => edit(row, axis, value)}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {positions.length > lanes && lanes >= MIN_ELEMENTS && (
        <Button
          type="button"
          className={`${BTN_SM} self-end`}
          title="Drop rows with no lane wired"
          onClick={() => onChange({ kind: "explicit", positions: positions.slice(0, lanes) })}
        >
          Trim
        </Button>
      )}
    </div>
  );
}

function ArrayPreview({
  geometry,
  lanes,
  azimuthDeg,
}: {
  geometry: ArrayGeometry;
  lanes: number;
  azimuthDeg: number | null;
}) {
  const centre = PREVIEW_PX / 2;
  const points = previewPoints(geometry, lanes, PREVIEW_PX);
  const north = azimuthDeg === null ? null : polarPoint(-azimuthDeg, centre - 8, centre);
  return (
    <svg
      viewBox={`0 0 ${PREVIEW_PX} ${PREVIEW_PX}`}
      width={PREVIEW_PX}
      height={PREVIEW_PX}
      role="img"
      aria-label="Array layout"
      className="shrink-0 rounded-[3px] border border-line bg-well"
    >
      <path d={`M${centre} 1 L${centre - 3} 6 L${centre + 3} 6 Z`} className="fill-ink-faint">
        <title>Forward</title>
      </path>
      {north !== null && (
        <g className="stroke-accent/70">
          <line x1={centre} y1={centre} x2={north.x} y2={north.y} strokeDasharray="2 2" />
          <text
            x={north.x}
            y={north.y}
            className="fill-accent stroke-none font-mono text-[8px]"
            textAnchor="middle"
            dominantBaseline="middle"
          >
            N
          </text>
        </g>
      )}
      {points.map((point) => (
        <g key={point.lane}>
          <circle
            cx={point.x}
            cy={point.y}
            r={5}
            className="fill-port-array/30 stroke-port-array"
          />
          <text
            x={point.x}
            y={point.y}
            className="fill-ink font-mono text-[6px]"
            textAnchor="middle"
            dominantBaseline="central"
          >
            {point.lane}
          </text>
        </g>
      ))}
    </svg>
  );
}
