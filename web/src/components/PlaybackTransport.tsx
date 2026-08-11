// The transport of a device node that is a recording rather than a radio (CANVAS §3: a
// recording is opened the same way a radio is, so it is the same node — this is the one strip
// that differs). Play, stop, loop and a scrubbable position, driven off `DeviceSet.playback`.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { controlPlayback, STATE_KEY } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, PlaybackAction, PlaybackStatus } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { ICON_BTN } from "./controls";
import {
  formatClock,
  isLooping,
  LOOP_SETTING,
  playbackPositionAt,
  samplesToSeconds,
} from "./playback";
import { Slider } from "./Slider";

/** Fast enough that the bar slides rather than steps, and it only re-renders this strip. */
const TICK_MS = 200;

export function PlaybackTransport({ set, status }: { set: DeviceSet; status: PlaybackStatus }) {
  const queryClient = useQueryClient();
  const { applyPatch } = useDevicePatch();
  // While a scrub is in flight the bar follows the pointer, not the server: the position it
  // reports is a block behind, and letting it win mid-drag makes the handle fight the hand.
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
      <button
        type="button"
        className={ICON_BTN}
        aria-label={status.paused ? "Play" : "Pause"}
        aria-pressed={!status.paused}
        title={status.paused ? "Play" : "Pause"}
        onClick={() => drive.mutate({ action: status.paused ? "play" : "pause" })}
      >
        {status.paused ? "▶" : "❚❚"}
      </button>
      <button
        type="button"
        className={ICON_BTN}
        aria-label="Stop"
        title="Stop and return to the start"
        onClick={() => drive.mutate({ action: "stop" })}
      >
        ■
      </button>
      <button
        type="button"
        className={`${ICON_BTN} ${looping ? "text-accent" : ""}`}
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
        ↻
      </button>
      <Slider
        label="Playback position"
        className="min-w-0 flex-1"
        value={Math.round(position)}
        min={0}
        // A recording with no samples has nowhere to scrub to; a one-wide track keeps the
        // control from dividing by zero while still reading as empty.
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

/** The position between snapshots. Anchored on each reported value and advanced on the clock,
 * the way `RecordingReadout` runs its elapsed counter — the server emits a state change per
 * transport command, not per block. The tick stops while paused: there is nothing to advance,
 * and a re-render every 200ms to redraw the same number is pure waste. */
function useLivePosition(status: PlaybackStatus, sampleRate: number, looping: boolean): number {
  const [anchor, setAnchor] = useState(() => ({
    position: status.position_samples,
    at: Date.now(),
  }));
  const [now, setNow] = useState(() => Date.now());

  // Re-anchor as the server reports: adjusting state during render is cheaper than an effect,
  // which would paint one frame of the stale position first.
  if (anchor.position !== status.position_samples) {
    setAnchor({ position: status.position_samples, at: Date.now() });
    setNow(Date.now());
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
