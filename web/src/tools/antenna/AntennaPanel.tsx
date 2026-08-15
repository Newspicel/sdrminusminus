import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { ALERT, CHIP, LABEL, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { formatMhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { toolRunQuery } from "../../lib/api";
import type { AntennaDesign, AntennaPart, AntennaReport } from "../../lib/types";
import { AntennaView } from "./AntennaView";
import {
  antennaReport,
  antennaRequest,
  DESIGN_OPTIONS,
  defaultDesign,
  formatImpedance,
  formatLength,
  type LengthUnit,
  UNIT_OPTIONS,
  usesFeedline,
} from "./antenna";

export function AntennaPanel() {
  const [frequencyMhz, setFrequencyMhz] = useState(145.5);
  const [design, setDesign] = useState<AntennaDesign>(defaultDesign("dipole"));
  const [velocityFactor, setVelocityFactor] = useState(0.95);
  const [feedlineFactor, setFeedlineFactor] = useState(0.66);
  const [unit, setUnit] = useState<LengthUnit>("m");

  const request = useMemo(
    () =>
      antennaRequest({
        frequency_hz: frequencyMhz * 1e6,
        velocity_factor: velocityFactor,
        feedline_velocity_factor: feedlineFactor,
        design,
      }),
    [frequencyMhz, velocityFactor, feedlineFactor, design],
  );
  const run = useQuery(toolRunQuery(request));
  const report = antennaReport(run.data);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-x-4 gap-y-3">
        <Labelled label="Frequency (MHz)">
          <NumberField
            label="Frequency in MHz"
            value={frequencyMhz}
            onCommit={setFrequencyMhz}
            min={0.01}
            max={300_000}
            step={0.005}
            className="w-32"
          />
        </Labelled>
        <Labelled label="Design">
          <Select
            label="Antenna design"
            value={design.type}
            options={DESIGN_OPTIONS}
            onChange={(type) => setDesign(defaultDesign(type))}
          />
        </Labelled>
        <Labelled label="Element factor">
          <NumberField
            label="Element velocity factor"
            value={velocityFactor}
            onCommit={setVelocityFactor}
            min={0.5}
            max={1}
            step={0.01}
          />
        </Labelled>
        {usesFeedline(design) && (
          <Labelled label="Coax factor">
            <NumberField
              label="Feedline velocity factor"
              value={feedlineFactor}
              onCommit={setFeedlineFactor}
              min={0.4}
              max={1}
              step={0.01}
            />
          </Labelled>
        )}
        <DesignSettings design={design} onChange={setDesign} />
        <Labelled label="Units">
          <Segmented label="Length units" value={unit} options={UNIT_OPTIONS} onChange={setUnit} />
        </Labelled>
      </div>

      {run.isError && <p className={ALERT}>{run.error.message}</p>}
      {report !== null && <Report report={report} unit={unit} />}
    </div>
  );
}

function Report({ report, unit }: { report: AntennaReport; unit: LengthUnit }) {
  const [highlight, setHighlight] = useState<string | null>(null);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2">
        <span className={CHIP}>
          <span className="text-ink-faint">λ</span>
          {formatLength(report.wavelength_m, unit)}
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">at</span>
          {formatMhz(report.frequency_hz)}
        </span>
        <span className={CHIP}>
          <span className="text-ink-faint">feedpoint</span>
          {formatImpedance(report.feedpoint_ohms)}
        </span>
        <span className={CHIP}>{report.balanced ? "Balanced — wants a balun" : "Unbalanced"}</span>
      </div>

      <AntennaView report={report} unit={unit} highlight={highlight} onHighlight={setHighlight} />

      <table className="w-full border-collapse">
        <thead>
          <tr className="border-b border-line">
            <th className={TABLE_HEAD}>Part</th>
            <th className={TABLE_HEAD}>Qty</th>
            <th className={TABLE_HEAD}>Length</th>
            <th className={TABLE_HEAD}>On the boom</th>
          </tr>
        </thead>
        <tbody>
          {report.parts.map((part) => (
            <Row
              key={part.name}
              part={part}
              unit={unit}
              lit={highlight === part.name}
              onHighlight={setHighlight}
            />
          ))}
        </tbody>
      </table>

      <ul className="flex flex-col gap-1.5">
        {report.notes.map((note) => (
          <li key={note} className="flex gap-2 text-xs text-ink-dim">
            <span aria-hidden className="text-ink-faint">
              ·
            </span>
            {note}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Row({
  part,
  unit,
  lit,
  onHighlight,
}: {
  part: AntennaPart;
  unit: LengthUnit;
  lit: boolean;
  onHighlight: (label: string | null) => void;
}) {
  return (
    <tr
      className={`border-b border-line/60 align-top ${lit ? "bg-panel-2" : ""}`}
      onPointerEnter={() => onHighlight(part.name)}
      onPointerLeave={() => onHighlight(null)}
    >
      <td className={`${TABLE_CELL} text-ink`}>
        {part.name}
        {part.detail != null && <p className="mt-0.5 font-sans text-ink-dim">{part.detail}</p>}
      </td>
      <td className={`${TABLE_CELL} text-ink-dim`}>{part.count > 1 ? `× ${part.count}` : ""}</td>
      <td className={`${TABLE_CELL} text-accent`}>{formatLength(part.length_m, unit)}</td>
      <td className={`${TABLE_CELL} text-ink-dim`}>
        {part.position_m == null ? "" : formatLength(part.position_m, unit)}
      </td>
    </tr>
  );
}

/** The controls a design adds to the common ones. A design with no choices adds nothing, which
 * is why this returns null rather than an empty group. */
function DesignSettings({
  design,
  onChange,
}: {
  design: AntennaDesign;
  onChange: (design: AntennaDesign) => void;
}) {
  switch (design.type) {
    case "inverted_v":
      return (
        <Labelled label="Apex angle (°)">
          <NumberField
            label="Apex angle in degrees"
            value={design.settings.apex_angle_deg ?? 120}
            onCommit={(apex_angle_deg) => onChange({ ...design, settings: { apex_angle_deg } })}
            min={60}
            max={180}
            step={5}
          />
        </Labelled>
      );
    case "ground_plane":
      return (
        <>
          <Labelled label="Radials">
            <NumberField
              label="Radial count"
              value={design.settings.radials ?? 4}
              onCommit={(radials) =>
                onChange({ ...design, settings: { ...design.settings, radials } })
              }
              min={1}
              max={32}
              step={1}
            />
          </Labelled>
          <Labelled label="Radial slope (°)">
            <NumberField
              label="Radial slope in degrees"
              value={design.settings.radial_slope_deg ?? 45}
              onCommit={(radial_slope_deg) =>
                onChange({ ...design, settings: { ...design.settings, radial_slope_deg } })
              }
              min={0}
              max={60}
              step={5}
            />
          </Labelled>
        </>
      );
    case "yagi":
      return (
        <>
          <Labelled label="Directors">
            <NumberField
              label="Director count"
              value={design.settings.directors ?? 2}
              onCommit={(directors) =>
                onChange({ ...design, settings: { ...design.settings, directors } })
              }
              min={0}
              max={20}
              step={1}
            />
          </Labelled>
          <Labelled label="Spacing (λ)">
            <NumberField
              label="Element spacing in wavelengths"
              value={design.settings.spacing_wavelengths ?? 0.2}
              onCommit={(spacing_wavelengths) =>
                onChange({ ...design, settings: { ...design.settings, spacing_wavelengths } })
              }
              min={0.1}
              max={0.4}
              step={0.01}
            />
          </Labelled>
        </>
      );
    default:
      return null;
  }
}

function Labelled({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <span className={LABEL}>{label}</span>
      {children}
    </div>
  );
}
