import { Button } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { segment, WELL } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Select } from "../../components/Select";
import { SettingRow } from "../../components/Settings";
import { RADAR_LIMITS as LIMITS, lowest, scaled } from "../../lib/limits";
import type { PassiveRadarParams } from "../../lib/types";
import {
  allElements,
  CFAR_OPTIONS,
  CFAR_WINDOW_OPTIONS,
  CLUTTER_OPTIONS,
  cfarKindOf,
  cleaningOf,
  cleaningOptions,
  elementOptions,
  GPU_OPTIONS,
  ILLUMINATOR_OPTIONS,
  illuminatorEdit,
  PFA_OPTIONS,
  pfaChoice,
  surveillanceOf,
  toggledSurveillance,
  WINDOW_OPTIONS,
  withReference,
} from "./radar";
import { SettingsFold } from "./SettingsFold";

type Edit = (next: Partial<PassiveRadarParams>) => void;

interface GroupProps {
  settings: PassiveRadarParams;
  edit: Edit;
}

const SMALL = "w-24";
const CLUTTER_STEP = 0.001;
const CMA_STEP = 0.0001;
const JERK_STEP = 0.1;
const OFFSET_KHZ = scaled(LIMITS.offset_hz, 1e-3);
const BANDWIDTH_KHZ = scaled(LIMITS.bandwidth_hz, 1e-3);
const OVERLAP_PERCENT = scaled(LIMITS.overlap, 100);
const RANK_PERCENT = scaled(LIMITS.os_rank, 100);

export function RadarSettings({
  settings,
  edit,
  lanes,
}: {
  settings: PassiveRadarParams;
  edit: Edit;
  lanes: number;
}) {
  return (
    <>
      <SourceGroup settings={settings} edit={edit} lanes={lanes} />
      <ClutterGroup settings={settings} edit={edit} />
      <ReferenceGroup settings={settings} edit={edit} />
      <DetectGroup settings={settings} edit={edit} />
      <TrackGroup settings={settings} edit={edit} />
    </>
  );
}

function SourceGroup({ settings, edit, lanes }: GroupProps & { lanes: number }) {
  const illuminator = settings.illuminator;
  return (
    <SettingsFold label="Source" open>
      <SettingRow label="Illuminator" title="Transmitter type">
        <Select
          label="Illuminator"
          value={illuminator.kind}
          options={ILLUMINATOR_OPTIONS}
          onChange={(kind) => edit(illuminatorEdit(kind, settings))}
        />
      </SettingRow>
      <SettingRow label="Offset" title="Illuminator minus array centre">
        <NumberField
          label="Offset"
          value={settings.offset_hz / 1_000}
          min={OFFSET_KHZ.min}
          max={OFFSET_KHZ.max}
          step={1}
          unit="kHz"
          className={SMALL}
          onCommit={(khz) => edit({ offset_hz: khz * 1_000 })}
        />
      </SettingRow>
      {"bandwidth_hz" in illuminator && (
        <SettingRow label="Bandwidth">
          <NumberField
            label="Bandwidth"
            value={illuminator.bandwidth_hz / 1_000}
            min={BANDWIDTH_KHZ.min}
            max={BANDWIDTH_KHZ.max}
            step={1}
            unit="kHz"
            className={SMALL}
            onCommit={(khz) =>
              edit({ illuminator: { kind: illuminator.kind, bandwidth_hz: khz * 1_000 } })
            }
          />
        </SettingRow>
      )}
      <SettingRow label="Reference" title="Element pointed at the transmitter">
        <Select
          label="Reference"
          value={settings.reference_element}
          options={elementOptions(lanes)}
          onChange={(element) => edit(withReference(settings.surveillance, element))}
        />
      </SettingRow>
      <SettingRow label="Surveillance" title="Elements that listen for echoes">
        <SurveillanceChips settings={settings} edit={edit} lanes={lanes} />
      </SettingRow>
      <SourceLimits settings={settings} edit={edit} />
    </SettingsFold>
  );
}

function SurveillanceChips({ settings, edit, lanes }: GroupProps & { lanes: number }) {
  const on = new Set(surveillanceOf(settings.surveillance, settings.reference_element, lanes));
  return (
    <span className={WELL}>
      {allElements(lanes).map((element) => (
        <Button
          key={element}
          type="button"
          aria-pressed={on.has(element)}
          aria-label={`Surveillance ${element + 1}`}
          disabled={element === settings.reference_element}
          title={element === settings.reference_element ? "Reference" : undefined}
          className={segment(on.has(element))}
          onClick={() =>
            edit({
              surveillance: toggledSurveillance(
                settings.surveillance,
                element,
                settings.reference_element,
                lanes,
              ),
            })
          }
        >
          {element + 1}
        </Button>
      ))}
    </span>
  );
}

function SourceLimits({ settings, edit }: GroupProps) {
  return (
    <>
      <SettingRow label="Range" title="Bistatic excess range">
        <NumberField
          label="Range"
          value={settings.max_range_km}
          min={lowest(LIMITS.max_range_km, 1)}
          max={LIMITS.max_range_km.max}
          step={1}
          unit="km"
          className={SMALL}
          onCommit={(km) => edit({ max_range_km: km })}
        />
      </SettingRow>
      <SettingRow label="Speed" title="Largest range rate">
        <NumberField
          label="Speed"
          value={settings.max_speed_mps}
          min={LIMITS.max_speed_mps.min}
          max={LIMITS.max_speed_mps.max}
          step={10}
          unit="m/s"
          className={SMALL}
          onCommit={(mps) => edit({ max_speed_mps: mps })}
        />
      </SettingRow>
      <SettingRow label="CPI" title="Coherent processing interval">
        <NumberField
          label="CPI"
          value={settings.cpi_ms}
          min={LIMITS.cpi_ms.min}
          max={LIMITS.cpi_ms.max}
          step={10}
          unit="ms"
          className={SMALL}
          onCommit={(ms) => edit({ cpi_ms: Math.round(ms) })}
        />
      </SettingRow>
      <SettingRow label="Overlap" title="Share of each CPI reused by the next">
        <NumberField
          label="Overlap"
          value={Math.round(settings.overlap * 100)}
          min={OVERLAP_PERCENT.min}
          max={OVERLAP_PERCENT.max}
          step={5}
          unit="%"
          className={SMALL}
          onCommit={(percent) => edit({ overlap: percent / 100 })}
        />
      </SettingRow>
      <SettingRow label="AoA" title="Angle of arrival per echo, needs calibration">
        <Checkbox label="AoA" checked={settings.aoa} onChange={(aoa) => edit({ aoa })} />
      </SettingRow>
      <SettingRow label="GPU">
        <Select
          label="GPU"
          value={settings.gpu}
          options={GPU_OPTIONS}
          onChange={(gpu) => edit({ gpu })}
          className={SMALL}
        />
      </SettingRow>
    </>
  );
}

function ClutterGroup({ settings, edit }: GroupProps) {
  const clutter = settings.clutter;
  const set = (next: Partial<PassiveRadarParams["clutter"]>) =>
    edit({ clutter: { ...clutter, ...next } });
  const method = clutter.method;
  const eca = method === "eca_batch" || method === "eca_sliding";
  const nlms = method === "nlms" || method === "block_nlms";
  return (
    <SettingsFold label="Clutter">
      <SettingRow label="Method">
        <Select
          label="Clutter method"
          value={method}
          options={CLUTTER_OPTIONS}
          onChange={(next) => set({ method: next })}
        />
      </SettingRow>
      {method !== "off" && (
        <>
          <SettingRow label="Reach" title="Clutter extent removed">
            <NumberField
              label="Reach"
              value={clutter.reach_km}
              min={LIMITS.reach_km.min}
              max={LIMITS.reach_km.max}
              step={0.1}
              unit="km"
              className={SMALL}
              onCommit={(km) => set({ reach_km: km })}
            />
          </SettingRow>
          <SettingRow label="Lead" title="Taps before zero delay">
            <NumberField
              label="Lead"
              value={clutter.lead}
              min={LIMITS.lead.min}
              max={LIMITS.lead.max}
              step={1}
              className={SMALL}
              onCommit={(lead) => set({ lead: Math.round(lead) })}
            />
          </SettingRow>
        </>
      )}
      {eca && <EcaRows settings={settings} edit={edit} />}
      {nlms && (
        <SettingRow label="Step">
          <NumberField
            label="Step"
            value={clutter.step}
            min={lowest(LIMITS.clutter_step, CLUTTER_STEP)}
            max={LIMITS.clutter_step.max}
            step={CLUTTER_STEP}
            className={SMALL}
            onCommit={(step) => set({ step })}
          />
        </SettingRow>
      )}
    </SettingsFold>
  );
}

function EcaRows({ settings, edit }: GroupProps) {
  const clutter = settings.clutter;
  const set = (next: Partial<PassiveRadarParams["clutter"]>) =>
    edit({ clutter: { ...clutter, ...next } });
  return (
    <>
      <SettingRow label="Doppler taps" title="Doppler shifts each side">
        <NumberField
          label="Doppler taps"
          value={clutter.doppler_taps}
          min={LIMITS.doppler_taps.min}
          max={LIMITS.doppler_taps.max}
          step={1}
          className={SMALL}
          onCommit={(taps) => set({ doppler_taps: Math.round(taps) })}
        />
      </SettingRow>
      {clutter.method === "eca_batch" ? (
        <>
          <SettingRow label="Batch">
            <NumberField
              label="Batch"
              value={clutter.batch_ms}
              min={LIMITS.batch_ms.min}
              max={LIMITS.batch_ms.max}
              step={1}
              unit="ms"
              className={SMALL}
              onCommit={(ms) => set({ batch_ms: ms })}
            />
          </SettingRow>
          <SettingRow label="Taper" title="Blend weights between batches">
            <Checkbox label="Taper" checked={clutter.taper} onChange={(taper) => set({ taper })} />
          </SettingRow>
        </>
      ) : (
        <SettingRow label="Extension">
          <NumberField
            label="Extension"
            value={clutter.extension_ms}
            min={LIMITS.extension_ms.min}
            max={LIMITS.extension_ms.max}
            step={1}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => set({ extension_ms: ms })}
          />
        </SettingRow>
      )}
      <SettingRow label="Loading" title="Diagonal loading">
        <NumberField
          label="Loading"
          value={clutter.loading}
          min={LIMITS.loading.min}
          max={LIMITS.loading.max}
          step={0.0001}
          className={SMALL}
          onCommit={(loading) => set({ loading })}
        />
      </SettingRow>
    </>
  );
}

function ReferenceGroup({ settings, edit }: GroupProps) {
  const cleaning = settings.reference;
  return (
    <SettingsFold label="Reference">
      <SettingRow label="Cleaning">
        <Select
          label="Cleaning"
          value={cleaning.kind}
          options={cleaningOptions(settings.illuminator.kind)}
          onChange={(kind) => edit({ reference: cleaningOf(kind, cleaning) })}
        />
      </SettingRow>
      {cleaning.kind === "cma" && (
        <>
          <SettingRow label="CMA taps">
            <NumberField
              label="CMA taps"
              value={cleaning.taps}
              min={LIMITS.cma_taps.min}
              max={LIMITS.cma_taps.max}
              step={1}
              className={SMALL}
              onCommit={(taps) => edit({ reference: { ...cleaning, taps: Math.round(taps) } })}
            />
          </SettingRow>
          <SettingRow label="CMA step">
            <NumberField
              label="CMA step"
              value={cleaning.step}
              min={lowest(LIMITS.cma_step, CMA_STEP)}
              max={LIMITS.cma_step.max}
              step={CMA_STEP}
              className={SMALL}
              onCommit={(step) => edit({ reference: { ...cleaning, step } })}
            />
          </SettingRow>
        </>
      )}
    </SettingsFold>
  );
}

function DetectGroup({ settings, edit }: GroupProps) {
  const cfar = settings.cfar;
  const set = (next: Partial<PassiveRadarParams["cfar"]>) => edit({ cfar: { ...cfar, ...next } });
  const plane = cfar.window === "plane";
  return (
    <SettingsFold label="Detect">
      <SettingRow label="CFAR">
        <Select
          label="CFAR"
          value={cfar.kind.kind}
          options={CFAR_OPTIONS}
          onChange={(kind) => set({ kind: cfarKindOf(kind, cfar.kind) })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Window" title="Range: across delay per Doppler row">
        <Select
          label="CFAR window"
          value={cfar.window}
          options={CFAR_WINDOW_OPTIONS}
          onChange={(window) => set({ window })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Pfa" title="False alarm chance per cell">
        <Select
          label="Pfa"
          value={pfaChoice(cfar.pfa)}
          options={PFA_OPTIONS}
          onChange={(pfa) => set({ pfa })}
          className={SMALL}
        />
      </SettingRow>
      <CellRows
        label=""
        guard={cfar.guard_range}
        train={cfar.train_range}
        maxTrain={LIMITS.train_range.max}
        onGuard={(guard_range) => set({ guard_range })}
        onTrain={(train_range) => set({ train_range })}
      />
      {plane && (
        <CellRows
          label="Doppler"
          guard={cfar.guard_doppler}
          train={cfar.train_doppler}
          maxTrain={LIMITS.train_doppler.max}
          onGuard={(guard_doppler) => set({ guard_doppler })}
          onTrain={(train_doppler) => set({ train_doppler })}
        />
      )}
      {cfar.kind.kind === "os" && (
        <SettingRow label="Rank" title="Ordered cell used as the noise estimate">
          <NumberField
            label="Rank"
            value={Math.round(cfar.kind.rank * 100)}
            min={RANK_PERCENT.min}
            max={RANK_PERCENT.max}
            step={1}
            unit="%"
            className={SMALL}
            onCommit={(percent) => set({ kind: { kind: "os", rank: percent / 100 } })}
          />
        </SettingRow>
      )}
      <DetectFloors settings={settings} edit={edit} />
    </SettingsFold>
  );
}

function CellRows({
  label,
  guard,
  train,
  maxTrain,
  onGuard,
  onTrain,
}: {
  label: string;
  guard: number;
  train: number;
  maxTrain: number;
  onGuard: (value: number) => void;
  onTrain: (value: number) => void;
}) {
  const guardLabel = label === "" ? "Guard" : `Guard ${label}`;
  const trainLabel = label === "" ? "Train" : `Train ${label}`;
  return (
    <>
      <SettingRow label={guardLabel} title="Cells skipped next to the cell tested">
        <NumberField
          label={guardLabel}
          value={guard}
          min={LIMITS.guard.min}
          max={LIMITS.guard.max}
          step={1}
          className={SMALL}
          onCommit={(value) => onGuard(Math.round(value))}
        />
      </SettingRow>
      <SettingRow label={trainLabel} title="Cells averaged for the noise">
        <NumberField
          label={trainLabel}
          value={train}
          min={LIMITS.train_range.min}
          max={maxTrain}
          step={1}
          className={SMALL}
          onCommit={(value) => onTrain(Math.round(value))}
        />
      </SettingRow>
    </>
  );
}

function DetectFloors({ settings, edit }: GroupProps) {
  const cfar = settings.cfar;
  const set = (next: Partial<PassiveRadarParams["cfar"]>) => edit({ cfar: { ...cfar, ...next } });
  return (
    <>
      <SettingRow label="Min Doppler" title="Slower rows count as clutter">
        <NumberField
          label="Min Doppler"
          value={cfar.min_doppler_hz}
          min={LIMITS.min_doppler_hz.min}
          max={LIMITS.min_doppler_hz.max}
          step={0.5}
          unit="Hz"
          className={SMALL}
          onCommit={(hz) => set({ min_doppler_hz: hz })}
        />
      </SettingRow>
      <SettingRow label="Min range">
        <NumberField
          label="Min range"
          value={cfar.min_range_km}
          min={LIMITS.min_range_km.min}
          max={LIMITS.min_range_km.max}
          step={0.5}
          unit="km"
          className={SMALL}
          onCommit={(km) => set({ min_range_km: km })}
        />
      </SettingRow>
      <SettingRow label="Min SNR">
        <NumberField
          label="Min SNR"
          value={cfar.min_snr_db}
          min={LIMITS.min_snr_db.min}
          max={LIMITS.min_snr_db.max}
          step={0.5}
          unit="dB"
          className={SMALL}
          onCommit={(db) => set({ min_snr_db: db })}
        />
      </SettingRow>
      <SettingRow label="Doppler window" title="Taper across batches">
        <Select
          label="Doppler window"
          value={settings.window}
          options={WINDOW_OPTIONS}
          onChange={(window) => edit({ window })}
        />
      </SettingRow>
    </>
  );
}

function TrackGroup({ settings, edit }: GroupProps) {
  const tracker = settings.tracker;
  const set = (next: Partial<PassiveRadarParams["tracker"]>) =>
    edit({ tracker: { ...tracker, ...next } });
  return (
    <SettingsFold label="Track">
      <SettingRow label="Start" title="Hits needed in a window of looks">
        <span className="flex items-center gap-1.5">
          <NumberField
            label="Start hits"
            value={tracker.confirm_hits}
            min={LIMITS.track_window.min}
            max={tracker.confirm_window}
            step={1}
            unit="M"
            className="w-16"
            onCommit={(hits) => set({ confirm_hits: Math.round(hits) })}
          />
          <span className="legend">of</span>
          <NumberField
            label="Start looks"
            value={tracker.confirm_window}
            min={tracker.confirm_hits}
            max={LIMITS.track_window.max}
            step={1}
            unit="N"
            className="w-16"
            onCommit={(looks) => set({ confirm_window: Math.round(looks) })}
          />
        </span>
      </SettingRow>
      <SettingRow label="Coast" title="Missed looks before a track ends">
        <NumberField
          label="Coast"
          value={tracker.coast_looks}
          min={LIMITS.coast_looks.min}
          max={LIMITS.coast_looks.max}
          step={1}
          className={SMALL}
          onCommit={(looks) => set({ coast_looks: Math.round(looks) })}
        />
      </SettingRow>
      <SettingRow label="Max accel">
        <NumberField
          label="Max accel"
          value={tracker.max_accel_mps2}
          min={LIMITS.max_accel_mps2.min}
          max={LIMITS.max_accel_mps2.max}
          step={1}
          unit="m/s²"
          className={SMALL}
          onCommit={(accel) => set({ max_accel_mps2: accel })}
        />
      </SettingRow>
      <SettingRow label="Gate" title="How far an echo may land from its track">
        <NumberField
          label="Gate"
          value={tracker.gate}
          min={LIMITS.gate.min}
          max={LIMITS.gate.max}
          step={0.1}
          className={SMALL}
          onCommit={(gate) => set({ gate })}
        />
      </SettingRow>
      <SettingRow label="Jerk" title="How fast targets may change acceleration">
        <NumberField
          label="Jerk"
          value={tracker.jerk}
          min={lowest(LIMITS.jerk, JERK_STEP)}
          max={LIMITS.jerk.max}
          step={JERK_STEP}
          className={SMALL}
          onCommit={(jerk) => set({ jerk })}
        />
      </SettingRow>
      <SettingRow label="Altitude" title="Target height when no ADS-B match">
        <NumberField
          label="Altitude"
          value={settings.assumed_altitude_m}
          min={LIMITS.altitude_m.min}
          max={LIMITS.altitude_m.max}
          step={100}
          unit="m"
          className={SMALL}
          onCommit={(metres) => edit({ assumed_altitude_m: metres })}
        />
      </SettingRow>
    </SettingsFold>
  );
}
