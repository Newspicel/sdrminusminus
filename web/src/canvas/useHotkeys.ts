import { useEffect, useLayoutEffect, useRef } from "react";

export interface HotkeyActions {
  tune: (steps: number) => void;
  stepBy: (direction: number) => void;
  focusDial: () => void;
  cycleMode: (direction: number) => void;
  adjustSquelch: (deltaDb: number) => void;
  toggleSquelch: () => void;
  selectChannel: (direction: number) => void;
  selectNode: (index: number) => void;
  togglePin: () => void;
  toggleView: () => void;
  toggleFull: () => void;
  undo: () => void;
  redo: () => void;
  showShortcuts: () => void;
}

export interface Binding {
  keys: string;
  what: string;
}

export const BINDINGS: readonly Binding[] = [
  { keys: "← →", what: "Tune down / up one step" },
  { keys: "Shift ← →", what: "Tune ten steps" },
  { keys: "[ ]", what: "Smaller / larger tune step" },
  { keys: "f", what: "Focus the dial — then Enter to type a frequency" },
  { keys: ", .", what: "Previous / next channel" },
  { keys: "m / M", what: "Cycle the selected channel's mode" },
  { keys: "- / + =", what: "Squelch down / up 2 dB" },
  { keys: "s", what: "Squelch on / off" },
  { keys: "1 – 9", what: "Select the nth node" },
  { keys: "p", what: "Pin / unpin the selected face on the rack" },
  { keys: "v", what: "Swap the patch and the rack" },
  { keys: "z", what: "Blow the selected face up to the whole window — Esc brings it back" },
  { keys: "Ctrl / ⌘ Z", what: "Undo the last change — the workspace's history, so for everyone" },
  { keys: "Ctrl / ⌘ Shift Z", what: "Redo (Ctrl / ⌘ Y too)" },
  { keys: "Ctrl / ⌘ C", what: "Copy the selected nodes and the wires between them" },
  { keys: "Ctrl / ⌘ V", what: "Paste them beside the originals — a copied radio names none" },
  { keys: "Backspace", what: "Delete the selected node or wire (right-click offers it too)" },
  { keys: "?", what: "This list" },
  { keys: "Esc", what: "Close an overlay or a menu, or drop the selection" },
];

export type Chord = Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">;

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
  const latest = useRef(actions);
  useLayoutEffect(() => {
    latest.current = actions;
  });

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (isTyping(event.target)) {
        return;
      }
      const act = latest.current;
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
        case "z":
          act.toggleFull();
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
      event.preventDefault();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
}

export function isTyping(target: EventTarget | null): boolean {
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
