// The keyboard layer (PLAN §10 "keyboard-first", DESIGN.md). One listener on the document,
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
  /** Zero-based node index from the number row — the patch in stored order. */
  selectNode: (index: number) => void;
  /** Pin or unpin the selected node's face on the rack (CANVAS §5). */
  togglePin: () => void;
  /** Swap between the patch and the rack. */
  toggleView: () => void;
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
  { keys: "f", what: "Focus the dial — then Enter to type a frequency" },
  { keys: ", .", what: "Previous / next channel" },
  { keys: "m / M", what: "Cycle the selected channel's mode" },
  // `=` is the unshifted `+` on a US layout and `+` is its own key elsewhere; both are bound, so
  // the sheet names both rather than the one that happens to be right for one keyboard.
  { keys: "- / + =", what: "Squelch down / up 2 dB" },
  { keys: "s", what: "Squelch on / off" },
  { keys: "Space", what: "Start / stop audio on the selected channel" },
  { keys: "1 – 9", what: "Select the nth node" },
  { keys: "p", what: "Pin / unpin the selected face on the rack" },
  { keys: "v", what: "Swap the patch and the rack" },
  { keys: "Backspace", what: "Delete the selected node or wire (right-click offers it too)" },
  { keys: "?", what: "This list" },
  { keys: "Esc", what: "Close an overlay or a menu" },
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
      // A focused button owns its own activation keys. Tuning stays available from anywhere,
      // but Space must press the button under focus rather than start audio.
      if ((event.key === " " || event.key === "Enter") && isActivatable(event.target)) {
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
        case "p":
          act.togglePin();
          break;
        case "v":
          act.toggleView();
          break;
        case " ":
          act.toggleAudio();
          break;
        case "?":
          act.showShortcuts();
          break;
        default:
          if (/^[1-9]$/.test(event.key)) {
            act.selectNode(Number(event.key) - 1);
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

function isActivatable(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.closest("button, a[href], summary") !== null;
}

/** A field being typed into owns every key it receives, and so does a control that has already
 * claimed the same keys — `data-hotkeys="off"` is how the dial keeps its own arrows, and how
 * every Base UI control that reads arrows, Home/End or typeahead letters keeps theirs. */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return (
    target.isContentEditable ||
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target.closest('[data-hotkeys="off"]') !== null
  );
}
