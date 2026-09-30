import { Button } from "../../components/BaseControls";
import { BTN, BTN_PRIMARY } from "../../components/controls";
import { coveredLabel, degreesLabel, SWEEP_TEXT, sweepOn } from "../../components/hunt";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Rose, type RoseNeedle, TRUE_MARKS } from "../../components/Rose";
import { SettingRow, Settings } from "../../components/Settings";
import { HUNT_LIMITS as LIMITS } from "../../lib/limits";
import type { HuntSweepParams, HuntSweep as Sweep } from "../../lib/types";

const ROSE_PX = 160;

export function sweepNeedles(sweep: Sweep | null): RoseNeedle[] {
  if (sweep === null) {
    return [];
  }
  const needles: RoseNeedle[] = [];
  if (sweep.peak_deg != null) {
    needles.push({ deg: sweep.peak_deg, weight: "primary" });
  }
  if (sweep.heading_deg != null) {
    needles.push({ deg: sweep.heading_deg, weight: "secondary" });
  }
  return needles;
}

export function HuntSweep({
  sweep,
  busy,
  onSweep,
  onEnd,
  onMark,
}: {
  sweep: Sweep | null;
  busy: boolean;
  onSweep: () => void;
  onEnd: () => void;
  onMark: () => void;
}) {
  const on = sweepOn(sweep);
  const state = SWEEP_TEXT[sweep?.state ?? "off"];
  const heading = sweep?.heading_deg ?? null;
  return (
    <div className="flex flex-col items-center gap-2 border-t border-line p-2">
      <Rose
        label="Sweep rose"
        size={ROSE_PX}
        marks={TRUE_MARKS}
        spectrum={sweep?.bins ?? []}
        needles={sweepNeedles(sweep)}
        dim={!on}
      />
      <Readout separated={false}>
        <ReadoutRow label="Heading">
          {heading === null ? (
            <span className="text-danger" title="Wire a phone GPS">
              None
            </span>
          ) : (
            degreesLabel(heading)
          )}
        </ReadoutRow>
        <ReadoutRow label="Peak">{degreesLabel(sweep?.peak_deg)}</ReadoutRow>
        <ReadoutRow label="Covered">{sweep === null ? "-" : coveredLabel(sweep)}</ReadoutRow>
        <ReadoutRow label="Sweep" title={state.title}>
          {state.label}
        </ReadoutRow>
      </Readout>
      <div className="flex flex-wrap justify-center gap-2">
        {on ? (
          <Button
            type="button"
            className={BTN}
            title="Back to warmer and colder"
            disabled={busy}
            onClick={onEnd}
          >
            End sweep
          </Button>
        ) : (
          <Button
            type="button"
            className={BTN_PRIMARY}
            title="Turn slowly all the way round"
            disabled={busy}
            onClick={onSweep}
          >
            Sweep
          </Button>
        )}
        <Button
          type="button"
          className={BTN}
          title="Send your heading as a bearing"
          disabled={busy}
          onClick={onMark}
        >
          Mark
        </Button>
      </div>
    </div>
  );
}

export function HuntSweepSettings({
  params,
  edit,
}: {
  params: HuntSweepParams;
  edit: (next: Partial<HuntSweepParams>) => void;
}) {
  return (
    <Settings>
      <SettingRow label="Beamwidth" title="Main lobe of the handheld antenna">
        <NumberField
          label="Beamwidth"
          unit="°"
          value={params.beamwidth_deg}
          min={LIMITS.beamwidth_deg.min}
          max={LIMITS.beamwidth_deg.max}
          step={1}
          onCommit={(beamwidth_deg) => edit({ beamwidth_deg })}
        />
      </SettingRow>
      <SettingRow label="Front/back">
        <NumberField
          label="Front/back"
          unit="dB"
          value={params.front_back_db}
          min={LIMITS.front_back_db.min}
          max={LIMITS.front_back_db.max}
          step={1}
          onCommit={(front_back_db) => edit({ front_back_db })}
        />
      </SettingRow>
      <SettingRow label="Min span" title="How far to turn before a bearing">
        <NumberField
          label="Min span"
          unit="°"
          value={params.min_span_deg}
          min={LIMITS.min_span_deg.min}
          max={LIMITS.min_span_deg.max}
          step={10}
          onCommit={(min_span_deg) => edit({ min_span_deg })}
        />
      </SettingRow>
      <SettingRow label="Min contrast" title="Peak over the weakest direction">
        <NumberField
          label="Min contrast"
          unit="dB"
          value={params.min_contrast_db}
          min={LIMITS.min_contrast_db.min}
          max={LIMITS.min_contrast_db.max}
          step={0.5}
          onCommit={(min_contrast_db) => edit({ min_contrast_db })}
        />
      </SettingRow>
      <SettingRow label="Mount" title="Antenna forward relative to the phone">
        <NumberField
          label="Mount"
          unit="°"
          value={params.mount_offset_deg}
          min={LIMITS.mount_offset_deg.min}
          max={LIMITS.mount_offset_deg.max}
          step={1}
          onCommit={(mount_offset_deg) => edit({ mount_offset_deg })}
        />
      </SettingRow>
    </Settings>
  );
}
