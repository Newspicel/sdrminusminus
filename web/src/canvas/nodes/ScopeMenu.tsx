import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button, Form, Input } from "../../components/BaseControls";
import { BTN_QUIET, FIELD, LABEL, SURFACE } from "../../components/controls";
import { formatHz } from "../../components/format";
import { BOOKMARKS_KEY, createBookmark } from "../../lib/api";
import { pushToast } from "../../lib/toasts";
import { pickText, type ScopePick } from "./scopePick";

/** Where on the plot the menu is anchored, as screen fractions of it. */
export interface ScopeMenuAt {
  x: number;
  y: number;
}

/**
 * What right-clicking a frequency on the spectrum offers.
 *
 * Anchored to the pointer but kept inside the face, like the band ruler's identify card: a menu
 * that hangs off the edge of a node is unreadable, and a node is not a viewport. Marked as plot
 * chrome so the plot's own pointer handlers decline it — the plot captures the pointer to pan and
 * to tune, and a capture on the ancestor would retarget every press in here.
 */
export function ScopeMenu({
  pick,
  at,
  draft,
  onTune,
  onChannel,
  onClose,
}: {
  pick: ScopePick;
  at: ScopeMenuAt;
  /** The label and mode a bookmark saved here opens with (`bookmarkDraft`). */
  draft: { label: string; mode: string | null };
  onTune: () => void;
  /** Hand the frequency to the mode picker, which is a dialog of its own (`ChannelPicker`). */
  onChannel: () => void;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const menuRef = useRef<HTMLDivElement>(null);
  const [label, setLabel] = useState<string | null>(null);
  const text = pickText(pick);

  // `navigator.clipboard` is absent outside a secure context, so a copy that never happened has
  // to surface rather than leave the operator pasting whatever they copied last.
  const copy = (what: string, value: string): void => {
    void (async () => {
      try {
        await navigator.clipboard.writeText(value);
        pushToast(`${what} copied: ${value}`, "info");
        onClose();
      } catch (error) {
        pushToast(error instanceof Error ? error.message : String(error));
      }
    })();
  };

  const save = useMutation({
    mutationFn: () =>
      createBookmark({
        label: (label ?? draft.label).trim(),
        freq_hz: pick.hz,
        mode: draft.mode,
      }),
    onError: (error: Error) => pushToast(error.message),
    onSuccess: onClose,
    onSettled: () => void queryClient.invalidateQueries({ queryKey: BOOKMARKS_KEY }),
  });

  // A menu that outlives what it was opened on is a menu that acts on the wrong frequency.
  useEffect(() => {
    const dismiss = (event: Event) => {
      if (event instanceof KeyboardEvent) {
        if (event.key === "Escape") {
          onClose();
        }
        return;
      }
      if (event.target instanceof Node && menuRef.current?.contains(event.target) === true) {
        return;
      }
      onClose();
    };
    window.addEventListener("keydown", dismiss);
    window.addEventListener("pointerdown", dismiss, { capture: true });
    return () => {
      window.removeEventListener("keydown", dismiss);
      window.removeEventListener("pointerdown", dismiss, { capture: true });
    };
  }, [onClose]);

  // A dialog the keyboard cannot reach is one whose Escape never fires and which a screen reader
  // announces but never enters, so opening it moves focus in and closing it hands focus back.
  useLayoutEffect(() => {
    const returnTo = document.activeElement;
    menuRef.current?.focus();
    return () => {
      if (returnTo instanceof HTMLElement) {
        returnTo.focus();
      }
    };
  }, []);

  return (
    <div
      ref={menuRef}
      tabIndex={-1}
      role="dialog"
      aria-label={`Frequency ${formatHz(pick.hz)}`}
      data-plot-chrome
      className={`${SURFACE} absolute z-30 flex w-56 -translate-x-1/2 flex-col p-1 outline-none`}
      style={{
        left: `clamp(7rem, ${at.x * 100}%, calc(100% - 7rem))`,
        top: `clamp(0px, ${at.y * 100}%, calc(100% - 12rem))`,
      }}
    >
      <span className="px-2 py-1 font-mono text-xs text-ink tabular-nums">{formatHz(pick.hz)}</span>

      <Button type="button" className={`${BTN_QUIET} w-full justify-start`} onClick={onTune}>
        Tune here
      </Button>
      <Button type="button" className={`${BTN_QUIET} w-full justify-start`} onClick={onChannel}>
        New channel here…
      </Button>

      <Button
        type="button"
        className={`${BTN_QUIET} w-full justify-start`}
        onClick={() => copy("Frequency", text.frequency)}
      >
        Copy frequency
      </Button>
      <Button
        type="button"
        className={`${BTN_QUIET} w-full justify-start`}
        onClick={() => copy("Offset", text.offset)}
      >
        Copy offset
      </Button>

      {label === null ? (
        <Button
          type="button"
          className={`${BTN_QUIET} w-full justify-start`}
          onClick={() => setLabel(draft.label)}
        >
          Mark this frequency…
        </Button>
      ) : (
        <Form
          className="flex flex-col gap-1 p-1"
          onSubmit={(event) => {
            event.preventDefault();
            if (label.trim() !== "") {
              save.mutate();
            }
          }}
        >
          <span className={LABEL}>Bookmark{draft.mode !== null && ` · ${draft.mode}`}</span>
          <Input
            autoFocus
            className={`${FIELD} w-full`}
            aria-label="Bookmark label"
            value={label}
            onChange={(event) => setLabel(event.target.value)}
          />
          <Button
            type="submit"
            className={`${BTN_QUIET} w-full justify-start`}
            disabled={label.trim() === "" || save.isPending}
          >
            Save bookmark
          </Button>
        </Form>
      )}
    </div>
  );
}
