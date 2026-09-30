import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { SettingRow } from "../../components/Settings";
import { BAND_SEED_HZ, type BandLimits, type Bounds, scaled } from "../../lib/limits";

const SMALL = "w-24";
const KHZ = 1_000;

export function BandRows({
  band,
  offsetHz,
  bandwidthHz,
  onOffset,
  onBandwidth,
}: {
  band: BandLimits;
  offsetHz: number;
  bandwidthHz: number | null;
  onOffset: (hz: number) => void;
  onBandwidth: (hz: number | null) => void;
}) {
  return (
    <>
      <OffsetRow limit={band.offset_hz} offsetHz={offsetHz} onOffset={onOffset} />
      <SettingRow label="Full band" title="Use every lane sample, no filter">
        <Checkbox
          label="Full band"
          checked={bandwidthHz === null}
          onChange={(full) => onBandwidth(full ? null : BAND_SEED_HZ)}
        />
      </SettingRow>
      {bandwidthHz !== null && (
        <WidthRow limit={band.bandwidth_hz} bandwidthHz={bandwidthHz} onBandwidth={onBandwidth} />
      )}
    </>
  );
}

export function WidthRow({
  limit,
  bandwidthHz,
  onBandwidth,
}: {
  limit: Bounds;
  bandwidthHz: number;
  onBandwidth: (hz: number) => void;
}) {
  const khz = scaled(limit, 1 / KHZ);
  return (
    <SettingRow label="Width" title="Band around the offset">
      <NumberField
        label="Width"
        value={bandwidthHz / KHZ}
        min={khz.min}
        max={khz.max}
        step={0.1}
        unit="kHz"
        className={SMALL}
        onCommit={(value) => onBandwidth(value * KHZ)}
      />
    </SettingRow>
  );
}

export function OffsetRow({
  limit,
  offsetHz,
  onOffset,
}: {
  limit: Bounds;
  offsetHz: number;
  onOffset: (hz: number) => void;
}) {
  const khz = scaled(limit, 1 / KHZ);
  return (
    <SettingRow label="Offset" title="From the array centre">
      <NumberField
        label="Offset"
        value={offsetHz / KHZ}
        min={khz.min}
        max={khz.max}
        step={0.1}
        unit="kHz"
        className={SMALL}
        onCommit={(value) => onOffset(value * KHZ)}
      />
    </SettingRow>
  );
}
