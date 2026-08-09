// Channel management for the active device set (PLAN §8, §10): add, tune, squelch, and listen
// per channel. Edits PATCH the full `ChannelSettings` with the same optimistic-cache contract
// as `useDevicePatch`, so sliders don't fight WS-driven refetches.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  channelTypesQuery,
  createChannel,
  deleteChannel,
  patchChannel,
  STATE_KEY,
} from "../lib/api";
import { useChannelAudio } from "../lib/audio/useChannelAudio";
import type {
  ChannelInfo,
  ChannelParams,
  ChannelSettings,
  DeviceSet,
  StateSnapshot,
} from "../lib/types";
import type { SdrSocket } from "../lib/ws";
import { defaultChannelSettings, mergeChannelSettings } from "./channelSettings";
import { BTN, FIELD } from "./controls";
import { formatKhz } from "./format";
import { NumberField } from "./NumberField";
import { useDebouncedCommit } from "./useDebouncedCommit";

const LABEL = "flex items-center gap-2 text-sm text-ink-dim";
const OFFSET_STEPS_HZ = [-25_000, -5_000, 5_000, 25_000];
const DEFAULT_SQUELCH_DB = -60;

type ChannelEdit =
  | Partial<ChannelSettings>
  | ((current: ChannelSettings) => Partial<ChannelSettings>);

export function ChannelsPanel({
  socket,
  deviceSet,
  selected,
  onSelect,
}: {
  socket: SdrSocket;
  deviceSet: DeviceSet;
  selected: number | null;
  onSelect: (ch: number | null) => void;
}) {
  const queryClient = useQueryClient();
  const types = useQuery(channelTypesQuery());
  const [newType, setNewType] = useState("nfm");
  const [error, setError] = useState<string | null>(null);

  const invalidateState = (): void => {
    void queryClient.invalidateQueries({ queryKey: STATE_KEY });
  };
  const createMut = useMutation({
    mutationFn: (settings: ChannelSettings) => createChannel(deviceSet.id, settings),
    onSuccess: (id) => {
      setError(null);
      onSelect(id);
    },
    onError: (e) => setError(e.message),
    onSettled: invalidateState,
  });
  const deleteMut = useMutation({
    mutationFn: (ch: number) => deleteChannel(deviceSet.id, ch),
    onError: (e) => setError(e.message),
    onSettled: invalidateState,
  });
  const patchMut = useMutation({
    mutationFn: (v: { ch: number; settings: ChannelSettings }) =>
      patchChannel(deviceSet.id, v.ch, v.settings),
    onSuccess: () => setError(null),
    // A rejected PATCH must be visible, not just snap the control back (CLAUDE.md: no silent
    // failure).
    onError: (e) => setError(e.message),
    onSettled: invalidateState,
  });

  // Same optimistic contract as `useDevicePatch`: cancel racing refetches, write the merged
  // settings synchronously so rapid edits accumulate, then PATCH the full object. The
  // function-edit form reads the optimistic value, so step buttons chain correctly.
  const applyEdit = (ch: number, edit: ChannelEdit): void => {
    void queryClient.cancelQueries({ queryKey: STATE_KEY });
    const prev = queryClient.getQueryData<StateSnapshot>(STATE_KEY);
    const current = prev?.device_sets
      .find((d) => d.id === deviceSet.id)
      ?.channels.find((c) => c.id === ch)?.settings;
    if (!prev || !current) {
      return;
    }
    const settings = mergeChannelSettings(
      current,
      typeof edit === "function" ? edit(current) : edit,
    );
    queryClient.setQueryData<StateSnapshot>(STATE_KEY, {
      ...prev,
      device_sets: prev.device_sets.map((d) =>
        d.id === deviceSet.id
          ? { ...d, channels: d.channels.map((c) => (c.id === ch ? { ...c, settings } : c)) }
          : d,
      ),
    });
    patchMut.mutate({ ch, settings });
  };

  const nameOf = (typeId: string): string =>
    types.data?.types.find((t) => t.type_id === typeId)?.name ?? typeId.toUpperCase();

  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <label className={LABEL}>
          Type
          <select
            className={FIELD}
            value={newType}
            onChange={(e) => setNewType(e.target.value)}
            aria-label="Channel type"
          >
            {(types.data?.types ?? []).map((t) => (
              <option key={t.type_id} value={t.type_id}>
                {t.name}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          className={BTN}
          disabled={createMut.isPending || defaultChannelSettings(newType) === null}
          onClick={() => {
            const settings = defaultChannelSettings(newType);
            if (settings) {
              createMut.mutate(settings);
            }
          }}
        >
          Add channel
        </button>
      </div>

      {error !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Rejected: {error}</span>
          <button type="button" className="shrink-0 underline" onClick={() => setError(null)}>
            dismiss
          </button>
        </div>
      )}

      {deviceSet.channels.length === 0 ? (
        <span className="text-sm text-ink-dim">No channels — add one to listen.</span>
      ) : (
        deviceSet.channels.map((c) => (
          <ChannelRow
            key={c.id}
            socket={socket}
            dsId={deviceSet.id}
            channel={c}
            name={nameOf(c.settings.params.type)}
            spanHz={deviceSet.settings.sample_rate ?? null}
            selected={selected === c.id}
            onSelect={() => onSelect(c.id)}
            onEdit={(edit) => applyEdit(c.id, edit)}
            onRemove={() => deleteMut.mutate(c.id)}
          />
        ))
      )}
    </div>
  );
}

function ChannelRow({
  socket,
  dsId,
  channel,
  name,
  spanHz,
  selected,
  onSelect,
  onEdit,
  onRemove,
}: {
  socket: SdrSocket;
  dsId: number;
  channel: ChannelInfo;
  name: string;
  spanHz: number | null;
  selected: boolean;
  onSelect: () => void;
  onEdit: (edit: ChannelEdit) => void;
  onRemove: () => void;
}) {
  const audio = useChannelAudio(socket, dsId, channel.id);
  // Any live intent — bound, still subscribing, or muted by a suspended output — must offer
  // Stop, or an in-flight/failed subscribe leaves the button inert (no silent failure).
  const engaged = audio.playing || audio.pending || audio.suspended;
  const settings = channel.settings;
  const offsetHz = settings.offset_hz ?? 0;
  const squelchDb = settings.squelch_db ?? null;
  // Remembered across off/on so re-enabling restores the last threshold.
  const [offSquelchDb, setOffSquelchDb] = useState(DEFAULT_SQUELCH_DB);
  const squelchSlider = useDebouncedCommit((db) => onEdit({ squelch_db: db }));
  const halfSpanKhz = spanHz !== null ? spanHz / 2000 : undefined;

  return (
    <div
      className={`flex flex-col gap-2 rounded border bg-panel px-3 py-2 ${
        selected ? "border-accent" : "border-line"
      }`}
    >
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <button
          type="button"
          className={`font-mono text-sm font-semibold max-md:min-h-10 ${
            selected ? "text-accent" : "text-ink"
          }`}
          onClick={onSelect}
          aria-pressed={selected}
        >
          {name}
        </button>

        <div className="flex flex-wrap items-center gap-1">
          {OFFSET_STEPS_HZ.map((step) => (
            <button
              key={step}
              type="button"
              className={`${BTN} font-mono tabular-nums`}
              onClick={() => onEdit((current) => ({ offset_hz: (current.offset_hz ?? 0) + step }))}
            >
              {step > 0 ? "+" : "−"}
              {Math.abs(step) / 1000}k
            </button>
          ))}
          <NumberField
            label="Offset (kHz)"
            value={offsetHz / 1000}
            min={halfSpanKhz !== undefined ? -halfSpanKhz : undefined}
            max={halfSpanKhz}
            step={0.5}
            onCommit={(khz) => onEdit({ offset_hz: Math.round(khz * 1000) })}
            className="w-24"
          />
          <span className="text-sm text-ink-dim">kHz</span>
        </div>

        <button
          type="button"
          className={`${BTN} ${audio.playing ? "border-accent text-accent" : ""}`}
          onClick={() => (engaged ? audio.stop() : audio.start())}
        >
          {engaged ? "Stop" : "Play"}
        </button>
        <label className={LABEL}>
          Vol
          <input
            type="range"
            className="w-20 accent-accent"
            min={0}
            max={1}
            step={0.02}
            value={audio.volume}
            onChange={(e) => audio.setVolume(Number(e.target.value))}
            aria-label="Volume"
          />
        </label>

        <button
          type="button"
          className={`${BTN} ml-auto hover:border-danger hover:text-danger`}
          onClick={onRemove}
        >
          Remove
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <label className={LABEL}>
          <input
            type="checkbox"
            className="accent-accent"
            checked={squelchDb !== null}
            onChange={(e) => {
              if (e.target.checked) {
                onEdit({ squelch_db: offSquelchDb });
              } else {
                setOffSquelchDb(squelchSlider.pending ?? squelchDb ?? DEFAULT_SQUELCH_DB);
                squelchSlider.cancel();
                onEdit({ squelch_db: null });
              }
            }}
          />
          Squelch
        </label>
        {squelchDb !== null && (
          <label className={LABEL}>
            <input
              type="range"
              className="w-28 accent-accent"
              min={-120}
              max={0}
              step={1}
              value={squelchSlider.pending ?? squelchDb}
              onChange={(e) => squelchSlider.change(Number(e.target.value))}
              aria-label="Squelch threshold (dB)"
            />
            <span className="w-14 text-right font-mono tabular-nums text-ink">
              {(squelchSlider.pending ?? squelchDb).toFixed(0)}{" "}
              <span className="text-ink-dim">dB</span>
            </span>
          </label>
        )}

        <ModeControls params={settings.params} onParams={(params) => onEdit({ params })} />
      </div>

      {audio.suspended && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Audio output suspended by the browser — no sound.</span>
          <button type="button" className="shrink-0 underline" onClick={audio.resumeOutput}>
            resume
          </button>
        </div>
      )}
      {audio.error !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Audio failed: {audio.error}</span>
          <button type="button" className="shrink-0 underline" onClick={audio.dismissError}>
            dismiss
          </button>
        </div>
      )}
    </div>
  );
}

function ModeControls({
  params,
  onParams,
}: {
  params: ChannelParams;
  onParams: (params: ChannelParams) => void;
}) {
  switch (params.type) {
    case "nfm":
      return (
        <label className={LABEL}>
          BW
          <BandwidthSelect
            valueHz={params.settings.bandwidth_hz ?? 12_500}
            optionsHz={[12_500, 25_000]}
            onCommit={(bandwidth_hz) =>
              onParams({ type: "nfm", settings: { ...params.settings, bandwidth_hz } })
            }
          />
        </label>
      );
    case "am":
      return (
        <>
          <label className={LABEL}>
            BW
            <BandwidthSelect
              valueHz={params.settings.bandwidth_hz ?? 10_000}
              optionsHz={[5_000, 8_000, 10_000]}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "am", settings: { ...params.settings, bandwidth_hz } })
              }
            />
          </label>
          <AgcToggle
            checked={params.settings.agc ?? true}
            onChange={(agc) => onParams({ type: "am", settings: { ...params.settings, agc } })}
          />
        </>
      );
    case "ssb": {
      const sideband = params.settings.sideband ?? "usb";
      return (
        <>
          <div
            className="flex overflow-hidden rounded border border-line"
            role="group"
            aria-label="Sideband"
          >
            {(["usb", "lsb"] as const).map((sb) => (
              <button
                key={sb}
                type="button"
                className={`px-2.5 py-1 font-mono text-sm uppercase transition-colors max-md:min-h-10 ${
                  sideband === sb ? "bg-panel-2 text-accent" : "text-ink-dim hover:text-ink"
                }`}
                aria-pressed={sideband === sb}
                onClick={() =>
                  onParams({ type: "ssb", settings: { ...params.settings, sideband: sb } })
                }
              >
                {sb}
              </button>
            ))}
          </div>
          <label className={LABEL}>
            BW
            <NumberField
              label="SSB bandwidth (Hz)"
              value={params.settings.bandwidth_hz ?? 2_700}
              min={200}
              max={10_000}
              step={100}
              onCommit={(bandwidth_hz) =>
                onParams({ type: "ssb", settings: { ...params.settings, bandwidth_hz } })
              }
              className="w-20"
            />
            Hz
          </label>
          <AgcToggle
            checked={params.settings.agc ?? true}
            onChange={(agc) => onParams({ type: "ssb", settings: { ...params.settings, agc } })}
          />
        </>
      );
    }
    case "wfm":
      return (
        <label className={LABEL}>
          De-emphasis
          <select
            className={FIELD}
            value={params.settings.deemphasis_us ?? 50}
            onChange={(e) =>
              onParams({
                type: "wfm",
                settings: { ...params.settings, deemphasis_us: Number(e.target.value) },
              })
            }
            aria-label="De-emphasis (µs)"
          >
            <option value={50}>50 µs</option>
            <option value={75}>75 µs</option>
          </select>
        </label>
      );
  }
}

function BandwidthSelect({
  valueHz,
  optionsHz,
  onCommit,
}: {
  valueHz: number;
  optionsHz: number[];
  onCommit: (hz: number) => void;
}) {
  return (
    <select
      className={FIELD}
      value={valueHz}
      onChange={(e) => onCommit(Number(e.target.value))}
      aria-label="Channel bandwidth"
    >
      {/* A preset can carry an off-list bandwidth; render it as selectable so the select
          doesn't lie (same rule as the device BW select). */}
      {!optionsHz.includes(valueHz) && (
        <option value={valueHz}>{formatKhz(valueHz)} (current)</option>
      )}
      {optionsHz.map((hz) => (
        <option key={hz} value={hz}>
          {formatKhz(hz)}
        </option>
      ))}
    </select>
  );
}

function AgcToggle({ checked, onChange }: { checked: boolean; onChange: (agc: boolean) => void }) {
  return (
    <label className={LABEL}>
      <input
        type="checkbox"
        className="accent-accent"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      AGC
    </label>
  );
}
