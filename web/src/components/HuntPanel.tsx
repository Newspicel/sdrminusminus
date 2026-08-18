import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { FaceBody, FaceEmpty, FaceFooter } from "../canvas/nodes/NodeShell";
import { STATE_KEY, startHunt, stopHunt } from "../lib/api";
import { type Clicker, startClicker } from "../lib/geiger";
import { useHuntStore } from "../lib/hunt";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, HuntSettings } from "../lib/types";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN_DANGER, BTN_PRIMARY } from "./controls";
import {
  BEARING_LABEL,
  bearing,
  formatHuntDb,
  formatStrength,
  huntRefusal,
  liveHunt,
} from "./hunt";
import { NumberField } from "./NumberField";
import { Readout, ReadoutRow } from "./Readout";
import { SettingGroup, SettingRow, Settings } from "./Settings";

const INTERVAL_MS = 50;

export function HuntPanel({
  active,
  empty,
  settings,
  clicks,
  onSettings,
  onClicks,
}: {
  active: DeviceSet | null;
  empty: string;
  settings: HuntSettings;
  clicks: boolean;
  onSettings: (settings: HuntSettings) => void;
  onClicks: (clicks: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const pushed = useHuntStore((s) => (active ? s.byDeviceSet[active.id] : undefined));
  const clearLive = useHuntStore((s) => s.clear);
  const freqMhz = settings.freq_hz / 1e6;
  const bwKhz = (settings.bw_hz ?? 12_500) / 1e3;
  const setFreqMhz = (mhz: number): void =>
    onSettings({ ...settings, freq_hz: Math.round(mhz * 1e6) });
  const setBwKhz = (khz: number): void => onSettings({ ...settings, bw_hz: Math.round(khz * 1e3) });

  const status = liveHunt(active, pushed);
  const strength = status?.strength ?? 0;
  const running = status !== null;
  const clicker = useRef<Clicker | null>(null);

  useEffect(() => {
    if (!running || !clicks) {
      clicker.current?.stop();
      clicker.current = null;
      return;
    }
    clicker.current ??= startClicker();
    return () => {
      clicker.current?.stop();
      clicker.current = null;
    };
  }, [running, clicks]);

  useEffect(() => {
    clicker.current?.setStrength(strength);
  }, [strength]);

  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });

  const startMut = useMutation({
    mutationFn: async (deviceSet: number) =>
      startHunt(deviceSet, { ...settings, interval_ms: INTERVAL_MS }),
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const stopMut = useMutation({
    mutationFn: stopHunt,
    onSuccess: (_status, deviceSet) => clearLive(deviceSet),
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  if (active === null) {
    return (
      <FaceBody>
        <FaceEmpty>{empty}</FaceEmpty>
      </FaceBody>
    );
  }

  const refusal = huntRefusal(active, Math.round(freqMhz * 1e6));
  const busy = startMut.isPending || stopMut.isPending;
  const heading = bearing(status);

  return (
    <>
      <FaceBody>
        {status !== null ? (
          <>
            <div className="p-2">
              <div
                className="relative h-3 overflow-hidden rounded-full bg-panel-2"
                role="meter"
                aria-label="Distance to the transmitter"
                aria-valuenow={Math.round(strength * 100)}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuetext={BEARING_LABEL[heading]}
              >
                <div
                  className={`absolute inset-y-0 left-0 rounded-full transition-[width] duration-100 ${
                    heading === "closing" || heading === "steady" ? "bg-accent" : "bg-accent-dim"
                  }`}
                  style={{ width: `${strength * 100}%` }}
                />
              </div>
            </div>
            <Readout separated={false}>
              <ReadoutRow label="Bearing">
                <span className={heading === "closing" ? "text-accent" : ""}>
                  {BEARING_LABEL[heading]}
                </span>
              </ReadoutRow>
              <ReadoutRow label="Strength">{formatStrength(status)}</ReadoutRow>
              <ReadoutRow label="Level">{formatHuntDb(status.level_db)}</ReadoutRow>
              <ReadoutRow label="Smoothed">{formatHuntDb(status.smooth_db)}</ReadoutRow>
              <ReadoutRow label="Walked">
                {formatHuntDb(status.floor_db)} → {formatHuntDb(status.best_db)}
              </ReadoutRow>
              <ReadoutRow label="Readings">{status.readings}</ReadoutRow>
              {status.error != null && (
                <ReadoutRow label="Fault">
                  <span className="text-danger">{status.error}</span>
                </ReadoutRow>
              )}
            </Readout>
            <Settings className="p-2">
              <SettingRow label="Clicks">
                <Checkbox label="Geiger clicks" checked={clicks} onChange={onClicks} />
              </SettingRow>
            </Settings>
          </>
        ) : (
          <>
            <Settings className="p-2">
              <SettingGroup label="Transmitter">
                <SettingRow label="Frequency">
                  <NumberField
                    label="Hunt frequency (MHz)"
                    value={freqMhz}
                    min={0}
                    step={0.001}
                    onCommit={setFreqMhz}
                    className="w-28"
                  />
                  <span className="legend">MHz</span>
                </SettingRow>
                <SettingRow label="Bandwidth">
                  <NumberField
                    label="Hunt bandwidth (kHz)"
                    value={bwKhz}
                    min={0.1}
                    step={0.1}
                    onCommit={setBwKhz}
                    className="w-28"
                  />
                  <span className="legend">kHz</span>
                </SettingRow>
                <SettingRow label="Clicks">
                  <Checkbox label="Geiger clicks" checked={clicks} onChange={onClicks} />
                </SettingRow>
              </SettingGroup>
            </Settings>
            <Readout>
              <ReadoutRow label="How it works">
                Walk with the radio. The clicks and the bar speed up as the signal gets stronger;
                the range they span is whatever ground you have covered so far.
              </ReadoutRow>
              {refusal !== null && (
                <ReadoutRow label="Refused">
                  <span className="text-danger">{refusal}</span>
                </ReadoutRow>
              )}
            </Readout>
          </>
        )}
      </FaceBody>

      <FaceFooter>
        {status !== null ? (
          <Button
            type="button"
            className={BTN_DANGER}
            disabled={busy}
            onClick={() => stopMut.mutate(active.id)}
          >
            Stop hunt
          </Button>
        ) : (
          <Button
            type="button"
            className={BTN_PRIMARY}
            disabled={busy || refusal !== null}
            onClick={() => startMut.mutate(active.id)}
          >
            Start hunt
          </Button>
        )}
      </FaceFooter>
    </>
  );
}
