import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import {
  ALERT,
  BTN,
  BTN_PRIMARY,
  CHIP,
  FIELD,
  LABEL,
  TABLE_CELL,
  TABLE_HEAD,
} from "../../components/controls";
import { formatHz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Slider } from "../../components/Slider";
import { toolRunQuery } from "../../lib/api";
import type { NanoVnaPoint, NanoVnaSweep, NanoVnaSweepRequest } from "../../lib/types";
import {
  formatDb,
  formatImpedance,
  formatVswr,
  gainDb,
  impedance,
  lowestVswrIndex,
  nanoVnaDevices,
  nanoVnaDevicesRequest,
  nanoVnaSweep,
  nanoVnaSweepRequest,
  phaseDeg,
  returnLossDb,
  vswr,
} from "./nanovna";

type Trace = "s11" | "s21";

const TRACE_OPTIONS = [
  { value: "s11", label: "S11 return loss" },
  { value: "s21", label: "S21 gain" },
] as const;

export function NanoVnaPanel() {
  const [port, setPort] = useState("");
  const [startMhz, setStartMhz] = useState(1);
  const [stopMhz, setStopMhz] = useState(30);
  const [points, setPoints] = useState(101);
  const [averages, setAverages] = useState(1);
  const [submitted, setSubmitted] = useState<NanoVnaSweepRequest | null>(null);
  const devicesQuery = useQuery(toolRunQuery(nanoVnaDevicesRequest()));
  const devices = nanoVnaDevices(devicesQuery.data);
  const effectivePort = port || devices[0]?.port || "";
  const sweepQuery = useQuery(
    toolRunQuery(submitted === null ? null : nanoVnaSweepRequest(submitted)),
  );
  const sweep = nanoVnaSweep(sweepQuery.data);

  function acquire() {
    const request = {
      port: effectivePort,
      start_hz: Math.round(startMhz * 1e6),
      stop_hz: Math.round(stopMhz * 1e6),
      points: Math.round(points),
      averages: Math.round(averages),
    };
    if (submitted !== null && JSON.stringify(submitted) === JSON.stringify(request)) {
      void sweepQuery.refetch();
    } else {
      setSubmitted(request);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-x-3 gap-y-3">
        <Labelled label="Serial port">
          <div className="flex gap-1.5">
            <Input
              aria-label="NanoVNA serial port"
              data-hotkeys="off"
              value={port}
              placeholder={devices[0]?.port ?? "/dev/ttyACM0 or COM3"}
              onChange={(event) => setPort(event.target.value)}
              className={`${FIELD} w-52`}
            />
            <Button
              type="button"
              className={BTN}
              disabled={devicesQuery.isFetching}
              onClick={() => void devicesQuery.refetch()}
            >
              Rescan
            </Button>
          </div>
        </Labelled>
        <Labelled label="Start (MHz)">
          <NumberField
            label="Sweep start in MHz"
            value={startMhz}
            onCommit={setStartMhz}
            min={0.01}
            max={6300}
            step={0.001}
            className="w-28"
          />
        </Labelled>
        <Labelled label="Stop (MHz)">
          <NumberField
            label="Sweep stop in MHz"
            value={stopMhz}
            onCommit={setStopMhz}
            min={0.01}
            max={6300}
            step={0.001}
            className="w-28"
          />
        </Labelled>
        <Labelled label="Points">
          <NumberField
            label="Sweep points"
            value={points}
            onCommit={setPoints}
            min={11}
            max={10_001}
            step={10}
            className="w-24"
          />
        </Labelled>
        <Labelled label="Averages">
          <NumberField
            label="Sweep averages"
            value={averages}
            onCommit={setAverages}
            min={1}
            max={16}
            step={1}
            className="w-20"
          />
        </Labelled>
        <Button
          type="button"
          className={BTN_PRIMARY}
          disabled={effectivePort.length === 0 || sweepQuery.isFetching || stopMhz <= startMhz}
          onClick={acquire}
        >
          {sweepQuery.isFetching ? "Sweeping…" : sweep === null ? "Acquire sweep" : "Sweep again"}
        </Button>
      </div>

      {devicesQuery.isError && <p className={ALERT}>{devicesQuery.error.message}</p>}
      {devices.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-ink-dim">
          <span>Detected</span>
          {devices.map((device) => (
            <Button
              key={device.port}
              type="button"
              className={CHIP}
              onClick={() => setPort(device.port)}
            >
              {device.label}
            </Button>
          ))}
        </div>
      )}
      {devices.length === 0 && !devicesQuery.isPending && !devicesQuery.isError && (
        <p className="text-xs text-ink-dim">
          No serial devices were found. Enter a port path directly after connecting the NanoVNA.
        </p>
      )}
      {sweepQuery.isError && <p className={ALERT}>{sweepQuery.error.message}</p>}
      {sweep !== null && <SweepReport sweep={sweep} />}
    </div>
  );
}

function SweepReport({ sweep }: { sweep: NanoVnaSweep }) {
  const [trace, setTrace] = useState<Trace>("s11");
  const [selected, setSelected] = useState<number | null>(null);
  const best = lowestVswrIndex(sweep.points);
  const marker = Math.min(selected ?? best, Math.max(0, sweep.points.length - 1));
  const point = sweep.points[marker];
  if (point === undefined) {
    return <p className={ALERT}>The NanoVNA returned an empty sweep.</p>;
  }
  const z = impedance(point.s11);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2">
        <span className={CHIP}>
          <span className="text-ink-faint">firmware</span>
          {sweep.firmware}
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">samples</span>
          {sweep.points.length}
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">best match</span>
          {formatVswr(vswr(sweep.points[best]?.s11 ?? point.s11))}
        </span>
      </div>
      <div className="flex flex-wrap items-end justify-between gap-3">
        <Segmented label="VNA trace" value={trace} options={TRACE_OPTIONS} onChange={setTrace} />
        <label className="flex min-w-52 flex-1 items-center gap-2 font-mono text-xs text-ink-dim">
          <span className={LABEL}>Marker</span>
          <Slider
            label="Sweep marker"
            min={0}
            max={Math.max(0, sweep.points.length - 1)}
            value={marker}
            onChange={setSelected}
            className="min-w-32 flex-1"
          />
        </label>
      </div>
      <NanoVnaPlot points={sweep.points} trace={trace} marker={marker} onMarker={setSelected} />
      <table className="w-full border-collapse">
        <thead>
          <tr className="border-b border-line">
            <th className={TABLE_HEAD}>Frequency</th>
            <th className={TABLE_HEAD}>VSWR</th>
            <th className={TABLE_HEAD}>Return loss</th>
            <th className={TABLE_HEAD}>S11 phase</th>
            <th className={TABLE_HEAD}>Impedance</th>
            <th className={TABLE_HEAD}>S21 gain</th>
          </tr>
        </thead>
        <tbody>
          <tr className="border-b border-line/60">
            <td className={`${TABLE_CELL} text-accent`}>{formatHz(point.frequency_hz)}</td>
            <td className={TABLE_CELL}>{formatVswr(vswr(point.s11))}</td>
            <td className={TABLE_CELL}>{formatDb(returnLossDb(point.s11))}</td>
            <td className={TABLE_CELL}>{phaseDeg(point.s11).toFixed(1)}°</td>
            <td className={TABLE_CELL}>{formatImpedance(z)}</td>
            <td className={TABLE_CELL}>{formatDb(gainDb(point.s21))}</td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

function NanoVnaPlot({
  points,
  trace,
  marker,
  onMarker,
}: {
  points: readonly NanoVnaPoint[];
  trace: Trace;
  marker: number;
  onMarker: (index: number) => void;
}) {
  const width = 720;
  const height = 260;
  const left = 58;
  const right = 16;
  const top = 16;
  const bottom = 34;
  const plotWidth = width - left - right;
  const plotHeight = height - top - bottom;
  const values = points.map((point) =>
    trace === "s11" ? returnLossDb(point.s11) : gainDb(point.s21),
  );
  const finite = values.filter(Number.isFinite);
  const low = finite.length === 0 ? -10 : Math.floor(Math.min(...finite) / 10) * 10;
  const rawHigh = finite.length === 0 ? 10 : Math.ceil(Math.max(...finite) / 10) * 10;
  const high = rawHigh <= low ? low + 10 : rawHigh;
  const x = (index: number) => left + (index / Math.max(1, points.length - 1)) * plotWidth;
  const y = (value: number) =>
    top + ((high - Math.max(low, Math.min(high, value))) / (high - low)) * plotHeight;
  const path = values
    .map(
      (value, index) => `${index === 0 ? "M" : "L"}${x(index).toFixed(2)},${y(value).toFixed(2)}`,
    )
    .join(" ");
  const markerPoint = points[marker];

  function moveMarker(event: React.PointerEvent<SVGSVGElement>) {
    const bounds = event.currentTarget.getBoundingClientRect();
    const localX = ((event.clientX - bounds.left) / bounds.width) * width;
    const fraction = Math.max(0, Math.min(1, (localX - left) / plotWidth));
    onMarker(Math.round(fraction * Math.max(0, points.length - 1)));
  }

  return (
    <svg
      role="img"
      aria-label={trace === "s11" ? "S11 return loss plot" : "S21 gain plot"}
      viewBox={`0 0 ${width} ${height}`}
      className="w-full rounded-[3px] border border-line bg-plot-bg"
      onPointerDown={moveMarker}
      onPointerMove={(event) => {
        if (event.buttons === 1) {
          moveMarker(event);
        }
      }}
    >
      {[0, 0.25, 0.5, 0.75, 1].map((fraction) => {
        const gridY = top + fraction * plotHeight;
        const value = high - fraction * (high - low);
        return (
          <g key={fraction}>
            <line
              x1={left}
              y1={gridY}
              x2={width - right}
              y2={gridY}
              stroke="var(--color-plot-grid)"
            />
            <text
              x={left - 8}
              y={gridY + 4}
              textAnchor="end"
              className="fill-plot-ink-dim text-[10px]"
            >
              {value.toFixed(0)}
            </text>
          </g>
        );
      })}
      <path
        d={path}
        fill="none"
        stroke="var(--color-accent)"
        strokeWidth={2}
        vectorEffect="non-scaling-stroke"
      />
      <line
        x1={x(marker)}
        y1={top}
        x2={x(marker)}
        y2={top + plotHeight}
        stroke="var(--color-plot-hold)"
        strokeWidth={1}
        vectorEffect="non-scaling-stroke"
      />
      <text x={left} y={height - 10} className="fill-plot-ink-dim text-[10px]">
        {formatHz(points[0]?.frequency_hz ?? 0)}
      </text>
      <text
        x={width - right}
        y={height - 10}
        textAnchor="end"
        className="fill-plot-ink-dim text-[10px]"
      >
        {formatHz(points.at(-1)?.frequency_hz ?? 0)}
      </text>
      {markerPoint !== undefined && (
        <text
          x={x(marker)}
          y={top + 12}
          textAnchor={marker > points.length / 2 ? "end" : "start"}
          className="fill-plot-hold text-[10px]"
        >
          {formatHz(markerPoint.frequency_hz)}
        </text>
      )}
    </svg>
  );
}

function Labelled({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <span className={LABEL}>{label}</span>
      {children}
    </div>
  );
}
