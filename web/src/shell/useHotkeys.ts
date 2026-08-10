// The keyboard layer (PLAN §10 "keyboard-first", DESIGN.md §8). One listener on the document,
// one table, and every binding also listed in the `?` overlay — a shortcut nobody can find is
// not a feature.
import { useEffect, useRef } from "react";

export interface HotkeyActions {
  /** Tune by `steps` of the current tune step. */
  tune: (steps: number) => void;
  /** Walk the tune-step ladder. */
  stepBy: (direction: number) => void;
  focusDial: () => void;
  cycleMode: (direction: number) => void;
  adjustSquelch: (deltaDb: number) => void;
  toggleSquelch: () => void;
  toggleAudio: () => void;
  selectChannel: (direction: number) => void;
  /** Zero-based tab index from the number row. */
  selectTab: (index: number) => void;
  showShortcuts: () => void;
}

export interface Binding {
  keys: string;
  what: string;
}

/** The table the overlay renders, in the order it is read. */
export const BINDINGS: readonly Binding[] = [
  { keys: "← →", what: "Tune down / up one step" },
  { keys: "Shift ← →", what: "Tune ten steps" },
  { keys: "[ ]", what: "Smaller / larger tune step" },
  { keys: "f", what: "Type a frequency into the dial" },
  { keys: ", .", what: "Previous / next channel" },
  { keys: "m / M", what: "Cycle the selected channel's mode" },
  { keys: "- =", what: "Squelch down / up 2 dB" },
  { keys: "s", what: "Squelch on / off" },
  { keys: "Space", what: "Start / stop audio on the selected channel" },
  { keys: "1 – 9", what: "Switch tab" },
  { keys: "?", what: "This list" },
  { keys: "Esc", what: "Close an overlay, or reset the spectrum view" },
];

export function useHotkeys(actions: HotkeyActions): void {
  // The listener is registered once; the actions it calls change every render as state moves.
  const latest = useRef(actions);
  latest.current = actions;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // Browser and OS shortcuts keep their meaning, and a field being typed into owns every
      // key it receives.
      if (event.ctrlKey || event.metaKey || event.altKey || isTyping(event.target)) {
        return;
      }
      const act = latest.current;
      const shift = event.shiftKey;
      switch (event.key) {
        case "ArrowLeft":
          act.tune(shift ? -10 : -1);
          break;
        case "ArrowRight":
          act.tune(shift ? 10 : 1);
          break;
        case "[":
          act.stepBy(-1);
          break;
        case "]":
          act.stepBy(1);
          break;
        case "f":
          act.focusDial();
          break;
        case ",":
          act.selectChannel(-1);
          break;
        case ".":
          act.selectChannel(1);
          break;
        case "m":
        case "M":
          act.cycleMode(shift ? -1 : 1);
          break;
        case "-":
          act.adjustSquelch(-2);
          break;
        case "=":
        case "+":
          act.adjustSquelch(2);
          break;
        case "s":
          act.toggleSquelch();
          break;
        case " ":
          act.toggleAudio();
          break;
        case "?":
          act.showShortcuts();
          break;
        default:
          if (/^[1-9]$/.test(event.key)) {
            act.selectTab(Number(event.key) - 1);
            break;
          }
          return;
      }
      // Only reached by a handled key: Space would scroll, `/` would open quick-find, and the
      // arrows would move whatever the browser thinks is focused.
      event.preventDefault();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
}

/** A field being typed into owns every key it receives, and so does a control that has already
 * claimed the same keys — `data-hotkeys="off"` is how the dial keeps its own arrows. */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return (
    target.isContentEditable ||
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    target.closest('[data-hotkeys="off"]') !== null
  );
}
