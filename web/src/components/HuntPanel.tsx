import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { HuntSweep, HuntSweepChips } from "../canvas/nodes/HuntSweep";
import { FaceBody, FaceFooter } from "../canvas/nodes/NodeShell";
import { markHunt, STATE_KEY, startHunt, stopHunt, sweepHunt } from "../lib/api";
import { type Clicker, startClicker } from "../lib/geiger";
import { decoderKey, useHuntStore } from "../lib/hunt";
import { clearAction, failAction } from "../lib/refusals";
import type { HuntStatus, HuntSweepParams } from "../lib/types";
import { Button } from "./BaseControls";
import { BTN, BTN_DANGER, BTN_PRIMARY } from "./controls";
import { Chips, ToggleChip } from "./face/Chips";
import { FaceFault } from "./face/Fault";
import { Readout, Readouts } from "./face/Readouts";
import { FaceStats, Stat } from "./face/Stats";
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
          <>
            {target !== null && (
              <Readouts ruled={false}>
                <Readout label="Hunting">
                  {target.channel.settings.params.type} at{" "}
                  {formatMhz(huntedHz(null, target.channel))}
                </Readout>
              </Readouts>
            )}
            {refusal !== null && <FaceFault message={refusal.label} detail={refusal.title} />}
          </>
        )}
        {status?.error != null && <FaceFault message={status.error} />}
        <Chips className="border-t border-line p-2">
          <ToggleChip label="Clicks" title="Geiger clicks" on={clicks} onChange={onClicks} />
          {positionWired && sweep !== null && <HuntSweepChips params={sweep} edit={onSweep} />}
        </Chips>
      </FaceBody>
      <FaceFooter>
        {status !== null && <HuntStats status={status} />}
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
      <Readouts ruled={false}>
        <Readout label="Hunting">{formatMhz(huntedHz(status, target.channel))}</Readout>
        <Readout label="Trend">
          <span className={heading === "closing" ? "text-accent" : ""}>{TREND_LABEL[heading]}</span>
        </Readout>
        <Readout label="Strength">{formatStrength(status)}</Readout>
        <Readout label="Level">{formatHuntDb(status.level_db)}</Readout>
        <Readout label="Smoothed">{formatHuntDb(status.smooth_db)}</Readout>
        <Readout label="Walked">
          {formatHuntDb(status.floor_db)} → {formatHuntDb(status.best_db)}
        </Readout>
      </Readouts>
    </>
  );
}

function HuntStats({ status }: { status: HuntStatus }) {
  const poseDrops = status.pose_drops ?? 0;
  return (
    <FaceStats>
      <Stat label="Readings" title="Level readings taken">
        {status.readings}
      </Stat>
      {poseDrops > 0 && (
        <Stat label="Pose drops" title="Poses refused, the queue was full" tone="danger">
          {poseDrops}
        </Stat>
      )}
    </FaceStats>
  );
}
