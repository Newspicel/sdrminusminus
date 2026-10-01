import { type ReactNode } from "react";
import type { ChannelDescriptor, ChannelSettings } from "../lib/types";
import type { ChannelEdit } from "../lib/useChannelPatch";
import { BlankerChip } from "./BlankerControl";
import { AUDIO_LIMITS, channelHasAudio, radioWindowHz, squelchMarginDb } from "./channelSettings";
import { inTuningRange, type Range } from "./dial";
import { FrequencyDial } from "./FrequencyDial";
import { Chips, NumberChip } from "./face/Chips";
import { formatMhz } from "./format";
import { type BroadcastStatus, ModeChips } from "./ModeChips";
import { TuneTo } from "./TuneTo";
import { TuningLock } from "./TuningLock";

export function ChannelDial({
  hz,
  descriptor,
  spanHz,
  centerHz,
  range,
  dialId,
  wheelTunes,
  locked: lockedByHand,
  heldBy,
  onTune,
  onLock,
}: {
  hz: number;
  descriptor: ChannelDescriptor | undefined;
  spanHz: number | null;
  centerHz: number | null;
  range: Range;
  dialId: string;
  wheelTunes: boolean;
  locked: boolean;
  heldBy: string | null;
  onTune: (hz: number) => void;
  onLock: (locked: boolean) => void;
}) {
  const heard = radioWindowHz(centerHz, spanHz, descriptor);
  const locked = lockedByHand || heldBy !== null;
  return (
    <div className="flex min-w-0 items-center gap-1">
      <FrequencyDial
        id={dialId}
        hz={hz}
        range={range}
        disabled={locked}
        wheelTunes={wheelTunes}
        onTune={onTune}
      />
      <span className="ml-auto flex shrink-0 items-center gap-1">
        <TuneTo
          title="Type a frequency to listen on"
          hz={hz}
          hint={
            heard === null
              ? `Reaches ${formatMhz(range.min)} – ${formatMhz(range.max)}`
              : `The radio hears ${formatMhz(heard.lowHz)} – ${formatMhz(heard.highHz)}`
          }
          resolve={(entered) => inTuningRange(entered, range)}
          disabled={locked}
          onTune={onTune}
        />
        <TuningLock
          locked={locked}
          held="Frequency locked"
          free="Lock frequency"
          hold={heldBy === null ? null : `Tuned by ${heldBy}. Unwire its control to tune by hand.`}
          onLock={onLock}
        />
      </span>
    </div>
  );
}

export function ChannelControls({
  settings,
  descriptor,
  onEdit,
  extra,
  broadcast,
}: {
  settings: ChannelSettings;
  descriptor: ChannelDescriptor | undefined;
  onEdit: (edit: ChannelEdit) => void;
  extra?: ReactNode;
  broadcast?: BroadcastStatus;
}) {
  const audio = channelHasAudio(descriptor);
  return (
    <Chips className="p-2">
      {audio && <MarginChip settings={settings} onEdit={onEdit} />}
      <ModeChips
        params={settings.params}
        broadcast={broadcast}
        limits={descriptor?.limits ?? []}
        onParams={(params) => onEdit({ params })}
      />
      {extra}
      {audio && (
        <BlankerChip
          blanker={settings.blanker ?? {}}
          onBlanker={(blanker) => onEdit({ blanker })}
        />
      )}
    </Chips>
  );
}

function MarginChip({
  settings,
  onEdit,
}: {
  settings: ChannelSettings;
  onEdit: (edit: ChannelEdit) => void;
}) {
  const marginDb = squelchMarginDb(settings.squelch);
  if (marginDb === null) {
    return null;
  }
  const { min, max } = AUDIO_LIMITS.squelchAutoMarginDb;
  return (
    <NumberChip
      label="Margin"
      value={marginDb}
      unit="dB"
      min={min}
      max={max}
      step={1}
      title="How far above the measured noise floor the squelch opens"
      onCommit={(margin_db) => onEdit({ squelch: { mode: "auto", margin_db } })}
    />
  );
}
