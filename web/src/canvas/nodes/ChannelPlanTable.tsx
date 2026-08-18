import { useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN_SM, TABLE_CELL, TABLE_HEAD } from "../../components/controls";
import { OptionalNumberField } from "../../components/NumberField";
import type { DmrChannelEntry, TrunkChannel } from "../../lib/types";
import {
  channelEntry,
  MAX_LOGICAL_CHANNEL,
  planSummary,
  trunkChannelSourceHint,
  trunkChannelSourceLabel,
  trunkChannelSourceTone,
  usable,
  withChannel,
  withoutChannel,
} from "./dmrTrunk";

export function ChannelPlanTable({
  label,
  rows,
  entries,
  found,
  following,
  onChange,
}: {
  label: string;
  rows: readonly TrunkChannel[];
  entries: readonly DmrChannelEntry[];
  found: readonly DmrChannelEntry[];
  following: ReadonlySet<number>;
  onChange: (entries: DmrChannelEntry[]) => void;
}) {
  const [lcn, setLcn] = useState<number | null>(null);
  const [mhz, setMhz] = useState<number | null>(null);
  const pending = channelEntry(lcn, mhz);
  const add = () => {
    if (pending === null) {
      return;
    }
    onChange(withChannel(entries, pending));
    setLcn(null);
    setMhz(null);
  };
  return (
    <div className="border-b border-line">
      <div className="flex items-center justify-between gap-2 px-2 pt-2">
        <span className="legend">{label}</span>
        {found.length > 0 && (
          <Button type="button" className={BTN_SM} onClick={() => onChange([...entries, ...found])}>
            Keep {found.length} found
          </Button>
        )}
      </div>
      <div className="max-h-48 overflow-y-auto">
        <table className="w-full border-collapse font-mono text-xs">
          <thead className="sticky top-0 bg-panel">
            <tr>
              <th className={`${TABLE_HEAD} text-right`}>LCN</th>
              <th className={`${TABLE_HEAD} text-right`}>MHz</th>
              <th className={TABLE_HEAD}>Known by</th>
              <th className={TABLE_HEAD}>
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((channel) => (
              <tr
                key={channel.logical_channel}
                className={following.has(channel.logical_channel) ? "bg-accent/10" : undefined}
              >
                <td className={`${TABLE_CELL} text-right text-ink`}>{channel.logical_channel}</td>
                <td
                  className={`${TABLE_CELL} text-right whitespace-nowrap ${
                    usable(channel.source) ? "text-ink" : "text-ink-faint"
                  }`}
                >
                  {(channel.freq_hz / 1e6).toFixed(4)}
                </td>
                <td
                  className={`${TABLE_CELL} ${trunkChannelSourceTone(channel.source)}`}
                  title={trunkChannelSourceHint(channel.source)}
                >
                  {trunkChannelSourceLabel(channel.source)}
                  {channel.source === "learned" && ` ${channel.confidence}%`}
                </td>
                <td className={`${TABLE_CELL} text-right`}>
                  {channel.source === "manual" && (
                    <Button
                      type="button"
                      className={BTN_SM}
                      aria-label={`Forget channel ${channel.logical_channel}`}
                      onClick={() => onChange(withoutChannel(entries, channel.logical_channel))}
                    >
                      Forget
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="flex items-center gap-2 px-2 pt-2">
        <OptionalNumberField
          label="Logical channel to add"
          placeholder="LCN"
          value={lcn}
          min={0}
          max={MAX_LOGICAL_CHANNEL}
          step={1}
          className="w-16"
          onCommit={setLcn}
        />
        <OptionalNumberField
          label="Frequency to add"
          placeholder="451.0125"
          value={mhz}
          min={0}
          step={0.0125}
          className="w-24"
          onCommit={setMhz}
        />
        <span className="legend">MHz</span>
        <Button type="button" className={BTN_SM} disabled={pending === null} onClick={add}>
          Add
        </Button>
      </div>
      <p className="px-2 py-2 text-xs text-ink-dim">{planSummary(rows)}</p>
    </div>
  );
}
