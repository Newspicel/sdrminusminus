import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Pause, Play, RotateCcw, Square } from "lucide-react";
import { useEffect, useState } from "react";
import { controlPlayback, STATE_KEY } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, PlaybackAction, PlaybackStatus } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { Button } from "./BaseControls";
import { ICON_BTN } from "./controls";
import { Icon } from "./Icon";
import {
  formatClock,
  isLooping,
  LOOP_SETTING,
  playbackPositionAt,
  samplesToSeconds,
} from "./playback";
import { Slider } from "./Slider";

const TICK_MS = 200;

export function PlaybackTransport({ set, status }: { set: DeviceSet; status: PlaybackStatus }) {
  const queryClient = useQueryClient();
  const { applyPatch } = useDevicePatch();
  const [scrub, setScrub] = useState<number | null>(null);

  const drive = useMutation({
    mutationFn: ({ action, position }: { action: PlaybackAction; position?: number }) =>
      controlPlayback(set.id, action, position),
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const sampleRate = set.settings.sample_rate ?? 0;
  const looping = isLooping(set);
  const live = useLivePosition(status, sampleRate, looping);
  const position = scrub ?? live;
  const atEnd = status.total_samples > 0 && position >= status.total_samples;

  return (
    <div className="flex items-center gap-2 border-b border-line p-2">
      <Button
        type="button"
        className={ICON_BTN}
        aria-label={status.paused ? "Play" : "Pause"}
        aria-pressed={!status.paused}
        title={status.paused ? "Play" : "Pause"}
        onClick={() => drive.mutate({ action: status.paused ? "play" : "pause" })}
      >
        <Icon glyph={status.paused ? Play : Pause} />
      </Button>
      <Button
        type="button"
        className={ICON_BTN}
        aria-label="Stop"
        title="Stop and return to the start"
        onClick={() => drive.mutate({ action: "stop" })}
      >
        <Icon glyph={Square} />
      </Button>
      <Button
        type="button"
        className={`${ICON_BTN} ${looping ? "bg-accent/15 text-accent" : ""}`}
        aria-label="Loop"
        aria-pressed={looping}
        title={
          looping
            ? "Looping: replays from the start"
            : atEnd
              ? "At the end — turn looping on, or stop and play again"
              : "Plays once, then holds silent"
        }
        onClick={() => applyPatch(set.id, { extra: [{ name: LOOP_SETTING, value: !looping }] })}
      >
        <Icon glyph={RotateCcw} />
      </Button>
      <Slider
        label="Playback position"
        className="min-w-0 flex-1"
        value={Math.round(position)}
        min={0}
        max={Math.max(1, status.total_samples)}
        onChange={setScrub}
        onCommit={(target) => {
          setScrub(null);
          drive.mutate({ action: "seek", position: target });
        }}
      />
      <span className="shrink-0 font-mono text-[10px] tabular-nums text-ink-dim">
        {formatClock(samplesToSeconds(position, sampleRate))}
        {" / "}
        {formatClock(samplesToSeconds(status.total_samples, sampleRate))}
      </span>
    </div>
  );
}

function useLivePosition(status: PlaybackStatus, sampleRate: number, looping: boolean): number {
  const [anchor, setAnchor] = useState(() => ({
    position: status.position_samples,
    at: Date.now(),
  }));
  const [now, setNow] = useState(() => Date.now());

  if (anchor.position !== status.position_samples) {
    setAnchor({ position: status.position_samples, at: now });
  }

  useEffect(() => {
    if (status.paused) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(timer);
  }, [status.paused]);

  return playbackPositionAt(
    { ...status, position_samples: anchor.position },
    now - anchor.at,
    sampleRate,
    looping,
  );
}
