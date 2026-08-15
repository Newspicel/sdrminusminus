import { useEffect, useLayoutEffect, useRef } from "react";

export interface HotkeyActions {
  /** Tune by `steps` of the current tune step. */
  tune: (steps: number) => void;
  /** Walk the tune-step ladder. */
  stepBy: (direction: number) => void;
  focusDial: () => void;
  cycleMode: (direction: number) => void;
  adjustSquelch: (deltaDb: number) => void;
  toggleSquelch: () => void;
  selectChannel: (direction: number) => void;
  /** Zero-based node index from the number row — the patch in stored order. */
  selectNode: (index: number) => void;
  togglePin: () => void;
  /** Swap between the patch and the rack. */
  toggleView: () => void;
  /** Step the shared workspace history back, or forward again. */
  undo: () => void;
  redo: () => void;
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
  { keys: "1 – 9", what: "Select the nth node" },
  { keys: "p", what: "Pin / unpin the selected face on the rack" },
  { keys: "v", what: "Swap the patch and the rack" },
  // The history is the workspace's, not this browser's, so the sheet says so where an operator
  // meets it: pressing undo here undoes for whoever else has the workspace open.
  { keys: "Ctrl / ⌘ Z", what: "Undo the last change — the workspace's history, so for everyone" },
  { keys: "Ctrl / ⌘ Shift Z", what: "Redo (Ctrl / ⌘ Y too)" },
  { keys: "Backspace", what: "Delete the selected node or wire (right-click offers it too)" },
  { keys: "?", what: "This list" },
  { keys: "Esc", what: "Close an overlay or a menu" },
];

/** The subset of a key event the chord rules read. */
export type Chord = Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">;

/** Which way this key steps the history, or `null` if it is not the chord.
 *
 * Both platforms' spellings are accepted — ⌘Z / Ctrl Z, their shifted twin, and the Windows Ctrl
 * Y — because the workspace is one server two operators can be on from different machines. */
export function historyStep(event: Chord): "undo" | "redo" | null {
  if (event.altKey || !(event.ctrlKey || event.metaKey)) {
    return null;
  }
  switch (event.key.toLowerCase()) {
    case "z":
      return event.shiftKey ? "redo" : "undo";
    case "y":
      return "redo";
    default:
      return null;
  }
}

export function useHotkeys(actions: HotkeyActions): void {
  // The listener is installed once and reads the actions through this ref, so a keypress always
  // runs the current closure. Written after commit rather than during render: React may replay
  // or discard a render, and a ref written by work that never commits would leak into the
  // listener.
  const latest = useRef(actions);
  useLayoutEffect(() => {
    latest.current = actions;
  });

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // A field being typed into owns every key it receives — including the modifier ones, so
      // that a text field keeps its own undo.
      if (isTyping(event.target)) {
        return;
      }
      const act = latest.current;
      // The one pair of chords the app claims. Taken before the guard below, which is what keeps
      // every other browser and OS shortcut meaning what it always did.
      const history = historyStep(event);
      if (history !== null) {
        (history === "undo" ? act.undo : act.redo)();
        event.preventDefault();
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) {
        return;
      }
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
      // Only reached by a handled key: `/` would open quick-find and the arrows would move
      // whatever the browser thinks is focused.
      event.preventDefault();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
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
