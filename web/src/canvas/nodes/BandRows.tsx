import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { SettingRow } from "../../components/Settings";

export const MAX_OFFSET_KHZ = 100_000;
export const MAX_BAND_KHZ = 20_000;
export const DEFAULT_BAND_HZ = 200_000;

const SMALL = "w-24";

export function BandRows({
  offsetHz,
  bandwidthHz,
  onOffset,
  onBandwidth,
  minBandHz = 1_000,
}: {
  offsetHz: number;
  bandwidthHz: number | null;
  onOffset: (hz: number) => void;
  onBandwidth: (hz: number | null) => void;
  minBandHz?: number;
}) {
  return (
    <>
      <OffsetRow offsetHz={offsetHz} onOffset={onOffset} />
      <SettingRow label="Full band" title="Use every lane sample, no filter">
        <Checkbox
          label="Full band"
          checked={bandwidthHz === null}
          onChange={(full) => onBandwidth(full ? null : DEFAULT_BAND_HZ)}
        />
      </SettingRow>
      {bandwidthHz !== null && (
        <WidthRow bandwidthHz={bandwidthHz} onBandwidth={onBandwidth} minBandHz={minBandHz} />
      )}
    </>
  );
}

export function WidthRow({
  bandwidthHz,
  onBandwidth,
  minBandHz = 1_000,
}: {
  bandwidthHz: number;
  onBandwidth: (hz: number) => void;
  minBandHz?: number;
}) {
  return (
    <SettingRow label="Width" title="Band around the offset">
      <NumberField
        label="Width"
        value={bandwidthHz / 1_000}
        min={minBandHz / 1_000}
        max={MAX_BAND_KHZ}
        step={0.1}
        unit="kHz"
        className={SMALL}
        onCommit={(khz) => onBandwidth(khz * 1_000)}
      />
    </SettingRow>
  );
}

export function OffsetRow({
  offsetHz,
  onOffset,
}: {
  offsetHz: number;
  onOffset: (hz: number) => void;
}) {
  return (
    <SettingRow label="Offset" title="From the array centre">
      <NumberField
        label="Offset"
        value={offsetHz / 1_000}
        min={-MAX_OFFSET_KHZ}
        max={MAX_OFFSET_KHZ}
        step={0.1}
        unit="kHz"
        className={SMALL}
        onCommit={(khz) => onOffset(khz * 1_000)}
      />
    </SettingRow>
  );
}
