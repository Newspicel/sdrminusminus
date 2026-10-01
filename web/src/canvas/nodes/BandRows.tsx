import { Checkbox } from "../../components/Checkbox";
import { ChipField, NumberChip, SettingChip } from "../../components/face/Chips";
import { NumberField } from "../../components/NumberField";
import { BAND_SEED_HZ, type BandLimits, type Bounds, scaled } from "../../lib/limits";

const KHZ = 1_000;

export function BandChips({
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
  const khz = scaled(band.bandwidth_hz, 1 / KHZ);
  return (
    <>
      <OffsetChip limit={band.offset_hz} offsetHz={offsetHz} onOffset={onOffset} />
      <SettingChip
        label="Width"
        value={bandwidthHz === null ? "full" : String(bandwidthHz / KHZ)}
        unit={bandwidthHz === null ? undefined : "kHz"}
        quiet={bandwidthHz === null}
        title="Band around the offset, or every lane sample"
      >
        {() => (
          <div className="flex flex-col gap-3">
            <ChipField label="Full band">
              <Checkbox
                label="Full band"
                checked={bandwidthHz === null}
                onChange={(full) => onBandwidth(full ? null : BAND_SEED_HZ)}
              />
            </ChipField>
            {bandwidthHz !== null && (
              <ChipField label="Width">
                <NumberField
                  className="min-w-0 flex-1"
                  label="Width"
                  value={bandwidthHz / KHZ}
                  min={khz.min}
                  max={khz.max}
                  step={0.1}
                  unit="kHz"
                  onCommit={(value) => onBandwidth(value * KHZ)}
                />
              </ChipField>
            )}
          </div>
        )}
      </SettingChip>
    </>
  );
}

export function WidthChip({
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
    <NumberChip
      label="Width"
      title="Band around the offset"
      value={bandwidthHz / KHZ}
      min={khz.min}
      max={khz.max}
      step={0.1}
      unit="kHz"
      onCommit={(value) => onBandwidth(value * KHZ)}
    />
  );
}

export function OffsetChip({
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
    <NumberChip
      label="Offset"
      title="From the array centre"
      value={offsetHz / KHZ}
      min={khz.min}
      max={khz.max}
      step={0.1}
      unit="kHz"
      quiet={offsetHz === 0}
      onCommit={(value) => onOffset(value * KHZ)}
    />
  );
}
