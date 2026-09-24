import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button } from "../components/BaseControls";
import { formatHz } from "../components/format";
import {
  BEARING_LABEL,
  bearing,
  formatHuntDb,
  HUNT_INTERVAL_MS,
  type HuntTarget,
  huntedHz,
  huntRefusal,
  liveHunt,
} from "../components/hunt";
import { STATE_KEY, startHunt, stopHunt } from "../lib/api";
import { type Clicker, startClicker } from "../lib/geiger";
import { decoderKey, useHuntStore } from "../lib/hunt";
import { toastError } from "../lib/toasts";
import type { MissionProps } from "./missions";

export function FoxHunt({ target }: MissionProps & { target: HuntTarget | null }) {
  const queryClient = useQueryClient();
  const set = target?.set ?? null;
  const pushed = useHuntStore((store) =>
    target === null ? undefined : store.byDecoder[decoderKey(target.set.id, target.channel.id)],
  );
  const clearLive = useHuntStore((store) => store.clear);
  const status = liveHunt(set, target?.channel.id ?? null, pushed);
  const strength = status?.strength ?? 0;
  const hz = huntedHz(status, target?.channel ?? null);
  const [clicks, setClicks] = useState(true);
  const clicker = useRef<Clicker | null>(null);
  const running = status !== null;

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
    onError: (error: Error) => toastError(error),
    onSettled: invalidate,
  });
  const stopMut = useMutation({
    mutationFn: stopHunt,
    onSuccess: (_status, decoder) => clearLive(decoder.deviceSet, decoder.channel),
    onError: (error: Error) => toastError(error),
    onSettled: invalidate,
  });

  const refusal = target === null ? "No decoder wired in." : huntRefusal(target);
  const busy = startMut.isPending || stopMut.isPending;

  return (
    <div className="flex h-full flex-col justify-center">
      <div className="px-3 py-2 text-center">
        <p className="font-mono text-3xl tabular-nums">{hz === null ? "-" : formatHz(hz)}</p>
        <p className="text-xs text-ink-dim">
          {status === null
            ? "not hunting"
            : `${formatHuntDb(status.smooth_db)} · ${BEARING_LABEL[bearing(status)]}`}
        </p>
      </div>
      <div className="px-3">
        <div
          className="h-16 w-full overflow-hidden rounded border border-line bg-bg"
          role="meter"
          aria-label="Signal strength"
          aria-valuenow={Math.round(strength * 100)}
        >
          <div
            className="h-full bg-accent transition-[width] duration-100"
            style={{ width: `${strength * 100}%` }}
          />
        </div>
      </div>
      {refusal !== null && <p className="px-3 pt-2 text-center text-danger text-xs">{refusal}</p>}
      <div className="flex justify-center gap-2 px-3 py-2">
        <Button
          type="button"
          disabled={target === null || busy || (!running && refusal !== null)}
          onClick={() => {
            if (target === null) {
              return;
            }
            if (running) {
              stopMut.mutate({ deviceSet: target.set.id, channel: target.channel.id });
            } else {
              startMut.mutate(target);
            }
          }}
          className={`rounded px-4 py-3 text-sm ${running ? "border border-line" : "bg-accent text-bg"}`}
        >
          {running ? "Stop hunt" : "Start hunt"}
        </Button>
        <Button
          type="button"
          onClick={() => setClicks((on) => !on)}
          className={`rounded px-4 py-3 text-sm ${clicks ? "bg-accent text-bg" : "border border-line"}`}
        >
          {clicks ? "Clicks on" : "Clicks off"}
        </Button>
      </div>
    </div>
  );
}
