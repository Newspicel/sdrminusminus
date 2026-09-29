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
  showHelp: () => void;
}

export type BindingGroup = "Tune" | "Channels" | "Canvas" | "Edit" | "General";

export interface Binding {
  keys: string;
  what: string;
  group: BindingGroup;
}

export const BINDINGS: readonly Binding[] = [
  { keys: "← →", what: "Tune one step", group: "Tune" },
  { keys: "Shift ← →", what: "Tune ten steps", group: "Tune" },
  { keys: "[ ]", what: "Smaller / larger step", group: "Tune" },
  { keys: "f", what: "Focus the dial, Enter to type", group: "Tune" },
  { keys: ", .", what: "Previous / next channel", group: "Channels" },
  { keys: "m / M", what: "Cycle analog mode", group: "Channels" },
  { keys: "- / +", what: "Squelch down / up 2 dB", group: "Channels" },
  { keys: "s", what: "Squelch on / off", group: "Channels" },
  { keys: "1 – 9", what: "Select the nth node", group: "Canvas" },
  { keys: "p", what: "Pin / unpin on the rack", group: "Canvas" },
  { keys: "v", what: "Swap patch and rack", group: "Canvas" },
  { keys: "z", what: "Fill the window, Esc returns", group: "Canvas" },
  { keys: "Ctrl / ⌘ Z", what: "Undo, shared by every client", group: "Edit" },
  { keys: "Ctrl / ⌘ Shift Z", what: "Redo", group: "Edit" },
  { keys: "Ctrl / ⌘ C", what: "Copy nodes and their wires", group: "Edit" },
  { keys: "Ctrl / ⌘ V", what: "Paste beside the originals", group: "Edit" },
  { keys: "Backspace", what: "Delete node or wire", group: "Edit" },
  { keys: "?", what: "Help", group: "General" },
  { keys: "Esc", what: "Close or deselect", group: "General" },
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
          act.showHelp();
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
