import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { FoldSection } from "../canvas/nodes/FoldSection";
import { HuntSweep, HuntSweepSettings } from "../canvas/nodes/HuntSweep";
import { FaceBody, FaceFooter } from "../canvas/nodes/NodeShell";
import { markHunt, STATE_KEY, startHunt, stopHunt, sweepHunt } from "../lib/api";
import { type Clicker, startClicker } from "../lib/geiger";
import { decoderKey, useHuntStore } from "../lib/hunt";
import { clearAction, failAction } from "../lib/refusals";
import type { HuntStatus, HuntSweepParams } from "../lib/types";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN, BTN_DANGER, BTN_PRIMARY } from "./controls";
import {
  formatHuntDb,
  formatStrength,
  type HuntTarget,
  huntedHz,
  huntRefusal,
  huntSettings,
  liveHunt,
  TREND_LABEL,
  type Trend,
  trend,
} from "./hunt";
import { Readout, ReadoutRow } from "./Readout";
import { SettingRow, Settings } from "./Settings";
import { formatMhz } from "./scanner";

export const HUNT_ACTION = "Hunt";
export const SWEEP_ACTION = "Sweep";
export const MARK_ACTION = "Mark";

export interface HuntPanelProps {
  node: string;
  target: HuntTarget | null;
  hint: string;
  clicks: boolean;
  onClicks: (clicks: boolean) => void;
  positionWired: boolean;
  sweep: HuntSweepParams | null;
  onSweep: (next: Partial<HuntSweepParams>) => void;
}

function useClicks(running: boolean, clicks: boolean, strength: number): void {
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
}

function decoderOf(hunted: HuntTarget) {
  return { deviceSet: hunted.set.id, channel: hunted.channel.id };
}

function useHuntActions(node: string, sweep: HuntSweepParams | null) {
  const queryClient = useQueryClient();
  const clearLive = useHuntStore((s) => s.clear);
  const invalidate = (): void => void queryClient.invalidateQueries({ queryKey: STATE_KEY });
  const settings = (hunted: HuntTarget) =>
    huntSettings(hunted.channel.id, node, sweep ?? undefined);
  const start = useMutation({
    mutationFn: (hunted: HuntTarget) => startHunt(decoderOf(hunted), settings(hunted)),
    onSuccess: () => clearAction(node, HUNT_ACTION),
    onError: (error) => failAction(node, HUNT_ACTION, error),
    onSettled: invalidate,
  });
  const stop = useMutation({
    mutationFn: (hunted: HuntTarget) => stopHunt(decoderOf(hunted)),
    onSuccess: (_status, hunted) => {
      clearAction(node, HUNT_ACTION);
      clearLive(hunted.set.id, hunted.channel.id);
    },
    onError: (error) => failAction(node, HUNT_ACTION, error),
    onSettled: invalidate,
  });
  const turn = useMutation({
    mutationFn: (hunted: HuntTarget) => sweepHunt(decoderOf(hunted), settings(hunted)),
    onSuccess: () => clearAction(node, SWEEP_ACTION),
    onError: (error) => failAction(node, SWEEP_ACTION, error),
    onSettled: invalidate,
  });
  const mark = useMutation({
    mutationFn: (hunted: HuntTarget) => markHunt(decoderOf(hunted)),
    onSuccess: () => clearAction(node, MARK_ACTION),
    onError: (error) => failAction(node, MARK_ACTION, error),
  });
  const busy = start.isPending || stop.isPending || turn.isPending || mark.isPending;
  return { start, stop, turn, mark, busy };
}

export function HuntPanel({
  node,
  target,
  hint,
  clicks,
  onClicks,
  positionWired,
  sweep,
  onSweep,
}: HuntPanelProps) {
  const pushed = useHuntStore((s) =>
    target ? s.byDecoder[decoderKey(target.set.id, target.channel.id)] : undefined,
  );
  const status = liveHunt(target?.set ?? null, target?.channel.id ?? null, pushed);
  useClicks(status !== null, clicks, status?.strength ?? 0);
  const actions = useHuntActions(node, sweep);
  const refusal = huntRefusal(target);
  const blocked = target === null || actions.busy || refusal !== null;
  return (
    <>
      <FaceBody title={target === null ? hint : undefined}>
        {status !== null && target !== null ? (
          <>
            <HuntReading status={status} target={target} />
            {positionWired && (
              <HuntSweep
                sweep={status.sweep ?? null}
                busy={actions.busy}
                onSweep={() => actions.turn.mutate(target)}
                onEnd={() => actions.start.mutate(target)}
                onMark={() => actions.mark.mutate(target)}
              />
            )}
          </>
        ) : (
          <Readout separated={false}>
            {target !== null && (
              <ReadoutRow label="Hunting">
                {target.channel.settings.params.type} at {formatMhz(huntedHz(null, target.channel))}
              </ReadoutRow>
            )}
            {refusal !== null && (
              <ReadoutRow label="Refused" title={refusal.title}>
                <span className="text-danger">{refusal.label}</span>
              </ReadoutRow>
            )}
          </Readout>
        )}
        <Settings className="p-2">
          <SettingRow label="Clicks">
            <Checkbox label="Geiger clicks" checked={clicks} onChange={onClicks} />
          </SettingRow>
        </Settings>
        {positionWired && sweep !== null && (
          <FoldSection label="Sweep">
            <HuntSweepSettings params={sweep} edit={onSweep} />
          </FoldSection>
        )}
      </FaceBody>
      <FaceFooter>
        {status !== null && target !== null ? (
          <Button
            type="button"
            className={BTN_DANGER}
            disabled={actions.busy}
            onClick={() => actions.stop.mutate(target)}
          >
            Stop hunt
          </Button>
        ) : (
          <>
            {positionWired && (
              <Button
                type="button"
                className={BTN}
                title="Turn slowly all the way round"
                disabled={blocked}
                onClick={() => target !== null && actions.turn.mutate(target)}
              >
                Sweep
              </Button>
            )}
            <Button
              type="button"
              className={BTN_PRIMARY}
              disabled={blocked}
              onClick={() => target !== null && actions.start.mutate(target)}
            >
              Start hunt
            </Button>
          </>
        )}
      </FaceFooter>
    </>
  );
}

function HuntMeter({ strength, heading }: { strength: number; heading: Trend }) {
  return (
    <div className="p-2">
      <div
        className="relative h-3 overflow-hidden rounded-full bg-panel-2"
        role="meter"
        aria-label="Distance to the transmitter"
        aria-valuenow={Math.round(strength * 100)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuetext={TREND_LABEL[heading]}
      >
        <div
          className={`absolute inset-y-0 left-0 rounded-full transition-[width] duration-100 ${
            heading === "closing" || heading === "steady" ? "bg-accent" : "bg-accent-dim"
          }`}
          style={{ width: `${strength * 100}%` }}
        />
      </div>
    </div>
  );
}

function HuntReading({ status, target }: { status: HuntStatus; target: HuntTarget }) {
  const heading = trend(status);
  return (
    <>
      <HuntMeter strength={status.strength ?? 0} heading={heading} />
      <Readout separated={false}>
        <ReadoutRow label="Hunting">{formatMhz(huntedHz(status, target.channel))}</ReadoutRow>
        <ReadoutRow label="Trend">
          <span className={heading === "closing" ? "text-accent" : ""}>{TREND_LABEL[heading]}</span>
        </ReadoutRow>
        <ReadoutRow label="Strength">{formatStrength(status)}</ReadoutRow>
        <ReadoutRow label="Level">{formatHuntDb(status.level_db)}</ReadoutRow>
        <ReadoutRow label="Smoothed">{formatHuntDb(status.smooth_db)}</ReadoutRow>
        <ReadoutRow label="Walked">
          {formatHuntDb(status.floor_db)} → {formatHuntDb(status.best_db)}
        </ReadoutRow>
        <ReadoutRow label="Readings">{status.readings}</ReadoutRow>
        {(status.pose_drops ?? 0) > 0 && (
          <ReadoutRow label="Pose drops" title="Poses refused, the queue was full">
            <span className="text-danger">{status.pose_drops}</span>
          </ReadoutRow>
        )}
        {status.error != null && (
          <ReadoutRow label="Fault">
            <span className="text-danger">{status.error}</span>
          </ReadoutRow>
        )}
      </Readout>
    </>
  );
}
