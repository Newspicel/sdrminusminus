/// The click rate a hunt maps onto, from "nothing here" to "you are standing on it".
export const SLOWEST_HZ = 1.5;
export const FASTEST_HZ = 40;
const CLICK_SECONDS = 0.004;
const TONE_HZ = 1_800;
const PEAK_GAIN = 0.28;
/// Never queue more than this far ahead, so a strength change is heard within a step or two.
const LOOKAHEAD_S = 0.15;

/// Turns closeness into a click rate the ear reads without looking: a Geiger counter's tell is
/// the rate, not the volume, which is what leaves the operator's eyes free to watch where they
/// are walking.
export function clickRateHz(strength: number): number {
  const clamped = Number.isFinite(strength) ? Math.min(Math.max(strength, 0), 1) : 0;
  return SLOWEST_HZ + (FASTEST_HZ - SLOWEST_HZ) * clamped * clamped;
}

export interface Clicker {
  setStrength(strength: number): void;
  stop(): void;
}

type ContextFactory = () => AudioContext;

/// Schedules clicks ahead of the clock rather than on a timer, because a `setTimeout` click track
/// jitters by exactly the amount that makes a rate hard to judge.
export function startClicker(
  makeContext: ContextFactory = () => new AudioContext(),
): Clicker | null {
  let context: AudioContext;
  try {
    context = makeContext();
  } catch {
    return null;
  }
  let strength = 0;
  let nextAt = context.currentTime + 0.05;
  let stopped = false;

  const click = (at: number): void => {
    const osc = context.createOscillator();
    const gain = context.createGain();
    osc.frequency.value = TONE_HZ;
    gain.gain.setValueAtTime(PEAK_GAIN, at);
    gain.gain.exponentialRampToValueAtTime(0.0001, at + CLICK_SECONDS);
    osc.connect(gain).connect(context.destination);
    osc.start(at);
    osc.stop(at + CLICK_SECONDS);
  };

  const pump = (): void => {
    if (stopped) {
      return;
    }
    const horizon = context.currentTime + LOOKAHEAD_S;
    while (nextAt < horizon) {
      if (nextAt > context.currentTime) {
        click(nextAt);
      }
      nextAt += 1 / clickRateHz(strength);
    }
    timer = setTimeout(pump, (LOOKAHEAD_S * 1000) / 2);
  };

  let timer: ReturnType<typeof setTimeout> | null = null;
  void context.resume().catch(() => undefined);
  pump();

  return {
    setStrength(next: number) {
      strength = next;
    },
    stop() {
      stopped = true;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      void context.close().catch(() => undefined);
    },
  };
}
