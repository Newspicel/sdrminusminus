import { X } from "lucide-react";
import { useState } from "react";
import { Button } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { BTN_SM, CHIP_SM } from "../../components/controls";
import { Icon } from "../../components/Icon";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import { BEAMFORMER_LIMITS as LIMITS } from "../../lib/limits";
import type { BeamformerParams } from "../../lib/types";
import { BandRows } from "./BandRows";
import {
  BEAM_MODES,
  type BeamSetting,
  laneOptions,
  nullLabel,
  referenceLanes,
  toggledReference,
  visibleSettings,
  withNull,
  withoutNull,
} from "./beamformer";
import { SettingsFold } from "./SettingsFold";

type Edit = (next: Partial<BeamformerParams>) => void;

interface Props {
  settings: BeamformerParams;
  edit: Edit;
  lanes: number;
}

const SMALL = "w-24";
const DEFAULT_AZIMUTH_DEG = 0;

export function BeamformerSettings({
  settings,
  edit,
  lanes,
  steerWired,
}: Props & { steerWired: boolean }) {
  const shown = visibleSettings(settings);
  return (
    <>
      <div className="border-t border-line p-2">
        <Settings>
          <SettingRow label="Mode">
            <Select
              label="Mode"
              value={settings.mode}
              options={BEAM_MODES}
              onChange={(mode) => edit({ mode })}
            />
          </SettingRow>
          {shown.has("steer") && <SteerRows settings={settings} edit={edit} wired={steerWired} />}
          {shown.has("nulls") && <NullRow settings={settings} edit={edit} />}
          {shown.has("main") && <CancellerRows settings={settings} edit={edit} lanes={lanes} />}
        </Settings>
      </div>
      <SettingsFold label="More">
        <TuningRows settings={settings} edit={edit} shown={shown} />
        <BlockRows settings={settings} edit={edit} shown={shown} />
        <BandRows
          band={LIMITS.band}
          offsetHz={settings.offset_hz}
          bandwidthHz={settings.bandwidth_hz}
          onOffset={(offset_hz) => edit({ offset_hz })}
          onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
        />
      </SettingsFold>
    </>
  );
}

function SteerRows({
  settings,
  edit,
  wired,
}: {
  settings: BeamformerParams;
  edit: Edit;
  wired: boolean;
}) {
  const steer = settings.steer;
  return (
    <>
      <SettingRow label="Steer">
        <Segmented
          label="Steer"
          value={steer.kind}
          options={[
            {
              value: "wired",
              label: "DF",
              title: wired ? "Follow the wired direction finder" : "Wire a DF to steer",
              disabled: !wired,
            },
            { value: "fixed", label: "Fixed", title: "One azimuth" },
          ]}
          onChange={(kind) =>
            edit({
              steer:
                kind === "wired"
                  ? { kind: "wired" }
                  : { kind: "fixed", azimuth_deg: DEFAULT_AZIMUTH_DEG, elevation_deg: 0 },
            })
          }
        />
      </SettingRow>
      {steer.kind === "fixed" && (
        <SettingRow label="Azimuth" title="From array forward">
          <NumberField
            label="Azimuth"
            value={steer.azimuth_deg}
            min={0}
            max={359.9}
            step={0.1}
            unit="°"
            className={SMALL}
            onCommit={(azimuth_deg) => edit({ steer: { ...steer, azimuth_deg } })}
          />
        </SettingRow>
      )}
    </>
  );
}

function NullRow({ settings, edit }: { settings: BeamformerParams; edit: Edit }) {
  const [draft, setDraft] = useState(0);
  const full = settings.nulls_deg.length >= LIMITS.nulls;
  return (
    <SettingRow label="Nulls" title="Azimuths to silence">
      {settings.nulls_deg.map((deg) => (
        <span key={deg} className={`${CHIP_SM} gap-1`}>
          {nullLabel(deg)}
          <Button
            type="button"
            aria-label={`Remove null ${Math.round(deg)}`}
            className="text-ink-faint hover:text-danger"
            onClick={() => edit({ nulls_deg: withoutNull(settings.nulls_deg, deg) })}
          >
            <Icon glyph={X} size={12} />
          </Button>
        </span>
      ))}
      <NumberField
        label="New null"
        value={draft}
        min={0}
        max={359.9}
        step={0.1}
        unit="°"
        className="w-20"
        disabled={full}
        onCommit={setDraft}
      />
      <Button
        type="button"
        className={BTN_SM}
        disabled={full}
        title={full ? `At most ${LIMITS.nulls}` : undefined}
        onClick={() => edit({ nulls_deg: withNull(settings.nulls_deg, draft) })}
      >
        Add
      </Button>
      <Checkbox
        label="Auto nulls"
        checked={settings.auto_nulls}
        onChange={(auto_nulls) => edit({ auto_nulls })}
      />
      <span className="legend">Auto</span>
    </SettingRow>
  );
}

function CancellerRows({ settings, edit, lanes }: Props) {
  const refs = new Set(referenceLanes(settings, lanes));
  return (
    <>
      <SettingRow label="Main" title="Lane the others are cancelled from">
        <Select
          label="Main"
          value={settings.main_lane}
          options={laneOptions(lanes)}
          onChange={(main_lane) =>
            edit({
              main_lane,
              reference_lanes: settings.reference_lanes.filter((lane) => lane !== main_lane),
            })
          }
          className="w-16"
        />
      </SettingRow>
      <SettingRow label="Refs" title="Lanes that hear only the interference">
        {laneOptions(lanes)
          .filter((option) => option.value !== settings.main_lane)
          .map((option) => (
            <span key={option.value} className="inline-flex items-center gap-1">
              <Checkbox
                label={`Reference ${option.label}`}
                checked={refs.has(option.value)}
                onChange={() =>
                  edit({ reference_lanes: toggledReference(settings, option.value, lanes) })
                }
              />
              <span className="font-mono text-xs">{option.label}</span>
            </span>
          ))}
      </SettingRow>
      <SettingRow label="Taps" title="Delays per reference lane">
        <NumberField
          label="Taps"
          value={settings.taps}
          min={LIMITS.taps.min}
          max={LIMITS.taps.max}
          step={1}
          className={SMALL}
          onCommit={(taps) => edit({ taps: Math.round(taps) })}
        />
      </SettingRow>
    </>
  );
}

function TuningRows({
  settings,
  edit,
  shown,
}: {
  settings: BeamformerParams;
  edit: Edit;
  shown: ReadonlySet<BeamSetting>;
}) {
  return (
    <>
      {shown.has("adaptation") && (
        <SettingRow label="Adapt">
          <Segmented
            label="Adapt"
            value={settings.adaptation}
            options={[
              { value: "nlms", label: "NLMS", title: "Light, follows slowly" },
              { value: "rls", label: "RLS", title: "Heavier, follows fast" },
            ]}
            onChange={(adaptation) => edit({ adaptation })}
          />
        </SettingRow>
      )}
      {shown.has("step") && (
        <SettingRow label="Step" title="How fast weights follow">
          <NumberField
            label="Step"
            value={settings.step}
            min={LIMITS.step.min}
            max={LIMITS.step.max}
            step={0.00001}
            className={SMALL}
            onCommit={(step) => edit({ step })}
          />
        </SettingRow>
      )}
      {shown.has("forget") && (
        <SettingRow label="Forget" title="Memory of the RLS solver">
          <NumberField
            label="Forget"
            value={settings.forget}
            min={LIMITS.forget.min}
            max={LIMITS.forget.max}
            step={0.00001}
            className={SMALL}
            onCommit={(forget) => edit({ forget })}
          />
        </SettingRow>
      )}
      {shown.has("loading") && (
        <SettingRow label="Loading" title="Diagonal loading, steadies the solve">
          <NumberField
            label="Loading"
            value={settings.loading}
            min={LIMITS.loading.min}
            max={LIMITS.loading.max}
            step={0.01}
            className={SMALL}
            onCommit={(loading) => edit({ loading })}
          />
        </SettingRow>
      )}
      {shown.has("noise") && (
        <SettingRow label="Noise">
          <Segmented
            label="Noise"
            value={settings.noise}
            options={[
              { value: "measured", label: "Measured", title: "Noise of each lane as measured" },
              { value: "white", label: "White", title: "Same noise on every lane" },
            ]}
            onChange={(noise) => edit({ noise })}
          />
        </SettingRow>
      )}
      {shown.has("timeout") && (
        <SettingRow label="Timeout" title="Hold the last bearing this long">
          <NumberField
            label="Timeout"
            value={settings.steer_timeout_ms}
            min={LIMITS.steer_timeout_ms.min}
            max={LIMITS.steer_timeout_ms.max}
            step={100}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => edit({ steer_timeout_ms: Math.round(ms) })}
          />
        </SettingRow>
      )}
    </>
  );
}

function BlockRows({
  settings,
  edit,
  shown,
}: {
  settings: BeamformerParams;
  edit: Edit;
  shown: ReadonlySet<BeamSetting>;
}) {
  return (
    <>
      {shown.has("crossfade") && (
        <SettingRow label="Fade" title="Blend old and new weights">
          <NumberField
            label="Fade"
            value={settings.crossfade_ms}
            min={LIMITS.crossfade_ms.min}
            max={LIMITS.crossfade_ms.max}
            step={1}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => edit({ crossfade_ms: Math.round(ms) })}
          />
        </SettingRow>
      )}
      {shown.has("update") && (
        <SettingRow label="Update" title="New weights this often">
          <NumberField
            label="Update"
            value={settings.update_ms}
            min={LIMITS.update_ms.min}
            max={LIMITS.update_ms.max}
            step={10}
            unit="ms"
            className={SMALL}
            onCommit={(ms) => edit({ update_ms: Math.round(ms) })}
          />
        </SettingRow>
      )}
      {shown.has("carry_over") && (
        <SettingRow label="Carry over" title="Share of the last estimate kept">
          <NumberField
            label="Carry over"
            value={settings.carry_over}
            min={LIMITS.carry_over.min}
            max={LIMITS.carry_over.max}
            step={0.01}
            className={SMALL}
            onCommit={(carry_over) => edit({ carry_over })}
          />
        </SettingRow>
      )}
    </>
  );
}
