import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { ALERT, BTN, BTN_PRIMARY, CHIP, FIELD, LABEL } from "../../components/controls";
import { formatHz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { Slider } from "../../components/Slider";
import { toolRunQuery } from "../../lib/api";
import type {
  NanoVnaCalibration,
  NanoVnaSweep,
  NanoVnaSweepRequest,
  NanoVnaSweepState,
} from "../../lib/types";
import { analyse, readouts } from "./analysis";
import { CalibrationPanel } from "./CalibrationPanel";
import { DeviceReport } from "./DeviceReport";
import {
  downloadText,
  exportFilename,
  sweepCsv,
  TOUCHSTONE_FORMATS,
  type TouchstoneFormat,
  touchstoneS1p,
  touchstoneS2p,
} from "./export";
import { MarkerReadout, SweepSummary } from "./MarkerReadout";
import {
  nanoVnaDescribeRequest,
  nanoVnaDevices,
  nanoVnaDevicesRequest,
  nanoVnaIgnoredPorts,
  nanoVnaReport,
  nanoVnaSweep,
  nanoVnaSweepRequest,
} from "./nanovna";
import { SmithChart } from "./SmithChart";
import { SweepChart } from "./SweepChart";
import { CHART_VIEWS, type ChartId, chartView } from "./traces";

type Tab = "measure" | "calibrate" | "device";

const TABS = [
  { value: "measure", label: "Measure" },
  { value: "calibrate", label: "Calibrate" },
  { value: "device", label: "Device" },
] as const;

const CHART_OPTIONS = CHART_VIEWS.map((view) => ({ value: view.id, label: view.label }));

const RANGE_PRESETS = [
  { label: "HF", startMhz: 0.05, stopMhz: 30 },
  { label: "6 m", startMhz: 50, stopMhz: 54 },
  { label: "2 m", startMhz: 144, stopMhz: 148 },
  { label: "70 cm", startMhz: 430, stopMhz: 440 },
  { label: "Full", startMhz: 0.05, stopMhz: 900 },
] as const;

export function NanoVnaPanel() {
  const [port, setPort] = useState("");
  const [startMhz, setStartMhz] = useState(1);
  const [stopMhz, setStopMhz] = useState(30);
  const [points, setPoints] = useState(101);
  const [averages, setAverages] = useState(1);
  const [tab, setTab] = useState<Tab>("measure");
  const [submitted, setSubmitted] = useState<NanoVnaSweepRequest | null>(null);
  const [calibration, setCalibration] = useState<NanoVnaCalibration | null>(null);

  const devicesQuery = useQuery(toolRunQuery(nanoVnaDevicesRequest()));
  const devices = nanoVnaDevices(devicesQuery.data);
  const ignored = nanoVnaIgnoredPorts(devicesQuery.data);
  const effectivePort = port || devices[0]?.port || "";

  const sweepQuery = useQuery(
    toolRunQuery(submitted === null ? null : nanoVnaSweepRequest(submitted)),
  );
  const sweep = nanoVnaSweep(sweepQuery.data);
  const describeQuery = useQuery(
    toolRunQuery(
      tab === "measure" || effectivePort === "" ? null : nanoVnaDescribeRequest(effectivePort),
    ),
  );
  const report = nanoVnaReport(describeQuery.data) ?? sweep?.device ?? null;

  const range: NanoVnaSweepState = {
    start_hz: Math.round(startMhz * 1e6),
    stop_hz: Math.round(stopMhz * 1e6),
    points: Math.round(points),
  };

  function acquire() {
    const request = { port: effectivePort, ...range, averages: Math.round(averages) };
    if (submitted !== null && JSON.stringify(submitted) === JSON.stringify(request)) {
      void sweepQuery.refetch();
    } else {
      setSubmitted(request);
    }
  }

  return (
    <div className="flex min-h-0 flex-col gap-4">
      <div className="flex flex-wrap items-end gap-x-3 gap-y-3">
        <Labelled label="Instrument">
          <div className="flex gap-1.5">
            <Input
              aria-label="NanoVNA serial port"
              data-hotkeys="off"
              value={port}
              placeholder={devices[0]?.port ?? "no NanoVNA found"}
              onChange={(event) => setPort(event.target.value)}
              className={`${FIELD} w-64`}
            />
            <Button
              type="button"
              className={BTN}
              disabled={devicesQuery.isFetching}
              onClick={() => void devicesQuery.refetch()}
            >
              {devicesQuery.isFetching ? "Scanning…" : "Rescan"}
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
          disabled={effectivePort === "" || sweepQuery.isFetching || stopMhz <= startMhz}
          onClick={acquire}
        >
          {sweepQuery.isFetching ? "Sweeping…" : sweep === null ? "Sweep" : "Sweep again"}
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <span className={LABEL}>Range</span>
        {RANGE_PRESETS.map((preset) => (
          <Button
            key={preset.label}
            type="button"
            className={CHIP}
            onClick={() => {
              setStartMhz(preset.startMhz);
              setStopMhz(preset.stopMhz);
            }}
          >
            {preset.label}
          </Button>
        ))}
      </div>

      <DeviceBar
        devices={devices}
        ignored={ignored}
        selected={effectivePort}
        pending={devicesQuery.isPending}
        error={devicesQuery.isError ? devicesQuery.error.message : null}
        onSelect={setPort}
      />

      <Segmented label="NanoVNA section" value={tab} options={TABS} onChange={setTab} />

      {sweepQuery.isError && <p className={ALERT}>{sweepQuery.error.message}</p>}
      {describeQuery.isError && tab !== "measure" && (
        <p className={ALERT}>{describeQuery.error.message}</p>
      )}

      {tab === "measure" &&
        (sweep === null ? (
          <p className="text-xs text-ink-dim">
            {effectivePort === ""
              ? "Connect a NanoVNA and rescan."
              : "Sweep to measure S11 and S21 across the range above."}
          </p>
        ) : (
          <SweepView sweep={sweep} />
        ))}

      {tab === "calibrate" && (
        <CalibrationPanel
          port={effectivePort}
          range={range}
          state={calibration ?? report?.calibration ?? null}
          onState={setCalibration}
        />
      )}

      {tab === "device" &&
        (report === null ? (
          <p className="text-xs text-ink-dim">
            {describeQuery.isFetching ? "Reading the instrument…" : "No instrument selected."}
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            <div>
              <Button
                type="button"
                className={BTN}
                disabled={describeQuery.isFetching}
                onClick={() => void describeQuery.refetch()}
              >
                {describeQuery.isFetching ? "Reading…" : "Re-read the instrument"}
              </Button>
            </div>
            <DeviceReport report={report} />
          </div>
        ))}
    </div>
  );
}

function DeviceBar({
  devices,
  ignored,
  selected,
  pending,
  error,
  onSelect,
}: {
  devices: ReturnType<typeof nanoVnaDevices>;
  ignored: string[];
  selected: string;
  pending: boolean;
  error: string | null;
  onSelect: (port: string) => void;
}) {
  if (error !== null) {
    return <p className={ALERT}>{error}</p>;
  }
  if (pending) {
    return <p className="text-xs text-ink-dim">Looking for a NanoVNA…</p>;
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5 text-xs text-ink-dim">
      {devices.length === 0 ? (
        <span>
          No NanoVNA found on any serial port. Connect one and rescan, or type its port above.
        </span>
      ) : (
        devices.map((device) => (
          <Button
            key={device.port}
            type="button"
            className={`${CHIP} ${device.port === selected ? "border-accent text-accent" : ""}`}
            onClick={() => onSelect(device.port)}
            title={[device.manufacturer, device.product, device.serial_number]
              .filter((field) => field !== undefined)
              .join(" · ")}
          >
            {device.label}
            {device.match_kind === "probable" && (
              <span className="text-ink-faint">unconfirmed</span>
            )}
          </Button>
        ))
      )}
      {ignored.length > 0 && (
        <span className="text-ink-faint" title={ignored.join("\n")}>
          {ignored.length} other serial {ignored.length === 1 ? "port" : "ports"} ignored
        </span>
      )}
    </div>
  );
}

function SweepView({ sweep }: { sweep: NanoVnaSweep }) {
  const [chart, setChart] = useState<ChartId>("magnitude");
  const [zoom, setZoom] = useState<{ from: number; to: number } | null>(null);
  const [marker, setMarker] = useState<number | null>(null);
  const [format, setFormat] = useState<TouchstoneFormat>("ri");

  const rows = useMemo(() => readouts(sweep.points), [sweep.points]);
  const analysis = useMemo(() => analyse(sweep.points), [sweep.points]);
  const visible = zoom === null ? rows : rows.slice(zoom.from, zoom.to + 1);
  const fallback = Math.max(0, (analysis.resonance?.index ?? 0) - (zoom?.from ?? 0));
  const lastIndex = Math.max(0, visible.length - 1);
  const active = Math.min(marker ?? fallback, lastIndex);
  const row = visible[active];
  const view = chartView(chart);

  if (row === undefined) {
    return <p className={ALERT}>The NanoVNA returned an empty sweep.</p>;
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className={CHIP}>
          <span className="text-ink-faint">points</span>
          {sweep.points.length}
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">averages</span>
          {sweep.averages}
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">took</span>
          {(sweep.elapsed_ms / 1000).toFixed(1)} s
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">correction</span>
          <span className={sweep.device.calibration.applied ? "text-ok" : "text-danger"}>
            {sweep.device.calibration.applied ? "on" : "off"}
          </span>
        </span>
        {sweep.device.bandwidth_hz !== undefined && (
          <span className={CHIP}>
            <span className="text-ink-faint">IF</span>
            {sweep.device.bandwidth_hz} Hz
          </span>
        )}
      </div>

      <div className="flex flex-wrap items-end justify-between gap-3">
        <Segmented label="Chart" value={chart} options={CHART_OPTIONS} onChange={setChart} />
        <div className="flex items-center gap-2">
          {zoom !== null && (
            <Button type="button" className={BTN} onClick={() => setZoom(null)}>
              Reset zoom
            </Button>
          )}
          <span className="font-mono text-[10px] text-ink-faint">
            drag to move the marker · shift-drag to zoom
          </span>
        </div>
      </div>

      {chart === "smith" ? (
        <div className="flex justify-center">
          <SmithChart rows={visible} marker={active} onMarker={setMarker} />
        </div>
      ) : (
        <SweepChart
          rows={visible}
          view={view}
          marker={active}
          onMarker={setMarker}
          onZoom={(from, to) =>
            setZoom({ from: (zoom?.from ?? 0) + from, to: (zoom?.from ?? 0) + to })
          }
        />
      )}

      <label className="flex items-center gap-2 font-mono text-xs text-ink-dim">
        <span className={LABEL}>Marker</span>
        <Slider
          label="Sweep marker"
          min={0}
          max={lastIndex}
          value={active}
          onChange={setMarker}
          className="min-w-32 flex-1"
        />
        <span className="w-24 text-right text-ink">{formatHz(row.frequencyHz)}</span>
      </label>

      <MarkerReadout row={row} />
      <SweepSummary analysis={analysis} />

      <div className="flex flex-wrap items-end gap-2 border-t border-line pt-3">
        <div className="flex flex-col gap-1">
          <span className={LABEL}>Touchstone format</span>
          <Select
            label="Touchstone number format"
            value={format}
            options={TOUCHSTONE_FORMATS}
            onChange={setFormat}
            className="w-48"
          />
        </div>
        <Button
          type="button"
          className={BTN}
          onClick={() =>
            downloadText(
              exportFilename(sweep, "s2p"),
              "application/octet-stream",
              touchstoneS2p(sweep, format, { recordedAt: new Date().toISOString() }),
            )
          }
        >
          Export .s2p
        </Button>
        <Button
          type="button"
          className={BTN}
          onClick={() =>
            downloadText(
              exportFilename(sweep, "s1p"),
              "application/octet-stream",
              touchstoneS1p(sweep, format, { recordedAt: new Date().toISOString() }),
            )
          }
        >
          Export .s1p
        </Button>
        <Button
          type="button"
          className={BTN}
          onClick={() => downloadText(exportFilename(sweep, "csv"), "text/csv", sweepCsv(sweep))}
        >
          Export CSV
        </Button>
      </div>
    </div>
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
