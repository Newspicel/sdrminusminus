import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { FaceBody, FaceFooter } from "../canvas/nodes/NodeShell";
import { STATE_KEY, startHunt, stopHunt } from "../lib/api";
import { type Clicker, startClicker } from "../lib/geiger";
import { decoderKey, useHuntStore } from "../lib/hunt";
import { pushToast } from "../lib/toasts";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN_DANGER, BTN_PRIMARY } from "./controls";
import {
  BEARING_LABEL,
  bearing,
  formatHuntDb,
  formatStrength,
  HUNT_INTERVAL_MS,
  type HuntTarget,
  huntedHz,
  huntRefusal,
  liveHunt,
} from "./hunt";
import { Readout, ReadoutRow } from "./Readout";
import { SettingRow, Settings } from "./Settings";
import { formatMhz } from "./scanner";

export function HuntPanel({
  target,
  hint,
  clicks,
  onClicks,
}: {
  target: HuntTarget | null;
  hint: string;
  clicks: boolean;
  onClicks: (clicks: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const set = target?.set ?? null;
  const pushed = useHuntStore((s) =>
    target ? s.byDecoder[decoderKey(target.set.id, target.channel.id)] : undefined,
  );
  const clearLive = useHuntStore((s) => s.clear);

  const status = liveHunt(set, target?.channel.id ?? null, pushed);
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
    mutationFn: async (hunted: HuntTarget) =>
      startHunt(
        { deviceSet: hunted.set.id, channel: hunted.channel.id },
        { channel: hunted.channel.id, interval_ms: HUNT_INTERVAL_MS },
      ),
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const stopMut = useMutation({
    mutationFn: stopHunt,
    onSuccess: (_status, decoder) => clearLive(decoder.deviceSet, decoder.channel),
    onError: (e) => pushToast(e.message),
    onSettled: invalidate,
  });

  const refusal = huntRefusal(target);
  const busy = startMut.isPending || stopMut.isPending;
  const heading = bearing(status);
  const hz = huntedHz(status, target?.channel ?? null);

  return (
    <>
      <FaceBody title={target === null ? hint : undefined}>
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
              <ReadoutRow label="Hunting">{formatMhz(hz)}</ReadoutRow>
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
            <Readout separated={false}>
              {target !== null && (
                <ReadoutRow label="Hunting">
                  {target.channel.settings.params.type} at {formatMhz(hz)}
                </ReadoutRow>
              )}
              {refusal !== null && (
                <ReadoutRow label="Refused">
                  <span className="text-danger">{refusal}</span>
                </ReadoutRow>
              )}
            </Readout>
            <Settings className="p-2">
              <SettingRow label="Clicks">
                <Checkbox label="Geiger clicks" checked={clicks} onChange={onClicks} />
              </SettingRow>
            </Settings>
          </>
        )}
      </FaceBody>

      <FaceFooter>
        {status !== null && target !== null ? (
          <Button
            type="button"
            className={BTN_DANGER}
            disabled={busy}
            onClick={() => stopMut.mutate({ deviceSet: target.set.id, channel: target.channel.id })}
          >
            Stop hunt
          </Button>
        ) : (
          <Button
            type="button"
            className={BTN_PRIMARY}
            disabled={target === null || busy || refusal !== null}
            onClick={() => target !== null && startMut.mutate(target)}
          >
            Start hunt
          </Button>
        )}
      </FaceFooter>
    </>
  );
}
