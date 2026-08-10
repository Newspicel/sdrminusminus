// The tuning dial (DESIGN.md §9) — the signature control of the UI. Every digit is its own
// target: scroll it, arrow it, or type over it. The arithmetic lives in `dial.ts`;
// this routes events and draws the place-value grouping.
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  type DialDigit,
  dialDigits,
  dialPlaces,
  parseFrequency,
  type Range,
  setDialDigit,
  stepDial,
} from "./dial";

/** The `f` shortcut focuses the dial from anywhere; an id is the one handle a global binding
 * can hold without threading a ref through the whole shell. */
export const DIAL_ID = "frequency-dial";

/**
 * The dial sizes off the node, never off the viewport: the operator resizes the node, so the
 * digits follow the `@container` the face puts around them (CANVAS §6 — the dial is the face of
 * every device node, at whatever size that node has been given). Every step is on DESIGN.md §3's
 * scale.
 */
const DIGIT_SIZE =
  "text-[16px] @min-[17rem]:text-[20px] @min-[22rem]:text-[26px] @min-[28rem]:text-[34px]";

export function FrequencyDial({
  hz,
  range,
  onTune,
  disabled = false,
  id = DIAL_ID,
}: {
  hz: number;
  range: Range;
  onTune: (hz: number) => void;
  /** Something else owns the tuning: a running scanner drives the radio and the server refuses a
   * client retune while it does (PLAN §18). The readout stays live; only the controls go. */
  disabled?: boolean;
  /** An id has to be unique, and the canvas draws one dial per device node, so a face passes its
   * own (`deviceDialId`). */
  id?: string;
}) {
  const places = useMemo(() => dialPlaces(range.max), [range.max]);
  const digits = dialDigits(hz, places);
  const [active, setActive] = useState(() => places.indexOf(6));
  const [draft, setDraft] = useState<string | null>(null);
  const dialRef = useRef<HTMLDivElement>(null);

  // A device swap can shorten the dial; an index past the end would address a place that is no
  // longer drawn, so every step would silently do nothing.
  const index = Math.min(active, places.length - 1);
  const place = places[index] ?? 6;

  // React marks its delegated wheel listener passive, so the scroll has to be intercepted
  // natively or the page scrolls while the dial tunes.
  useEffect(() => {
    const dial = dialRef.current;
    if (dial === null || draft !== null || disabled) {
      return;
    }
    const onWheel = (event: WheelEvent) => {
      const target = (event.target as HTMLElement).closest("[data-place]");
      const wheelPlace = Number(target?.getAttribute("data-place"));
      if (!Number.isFinite(wheelPlace)) {
        return;
      }
      event.preventDefault();
      setActive(places.indexOf(wheelPlace));
      onTune(stepDial(hz, wheelPlace, event.deltaY < 0 ? 1 : -1, range));
    };
    dial.addEventListener("wheel", onWheel, { passive: false });
    return () => dial.removeEventListener("wheel", onWheel);
  }, [hz, places, range, onTune, draft, disabled]);

  if (draft !== null) {
    return (
      <DirectEntry
        draft={draft}
        onDraft={setDraft}
        onCommit={(entered) => {
          setDraft(null);
          onTune(Math.min(range.max, Math.max(range.min, entered)));
        }}
      />
    );
  }

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const key = event.key;
    if (key === "ArrowUp" || key === "ArrowDown") {
      event.preventDefault();
      onTune(stepDial(hz, place, key === "ArrowUp" ? 1 : -1, range));
    } else if (key === "PageUp" || key === "PageDown") {
      event.preventDefault();
      onTune(stepDial(hz, place + 1, key === "PageUp" ? 1 : -1, range));
    } else if (key === "ArrowLeft" || key === "ArrowRight") {
      event.preventDefault();
      const next = index + (key === "ArrowRight" ? 1 : -1);
      setActive(Math.min(places.length - 1, Math.max(0, next)));
    } else if (key === "Home" || key === "End") {
      event.preventDefault();
      setActive(key === "Home" ? 0 : places.length - 1);
    } else if (/^[0-9]$/.test(key)) {
      event.preventDefault();
      onTune(setDialDigit(hz, place, Number(key), range));
      setActive(Math.min(places.length - 1, index + 1));
    } else if (key === "Enter") {
      // Direct entry is the keyboard's, and only the keyboard's: a pointer gesture that opened
      // it swallowed the second press of a double-click on a digit, which is the fastest way to
      // step one — the control fighting the gesture.
      event.preventDefault();
      setDraft("");
    }
  };

  return (
    // biome-ignore lint/a11y/useSemanticElements: a spinbutton over ten place-value targets is
    // the roving-tabindex pattern; a native input cannot address one decade at a time.
    <div
      ref={dialRef}
      id={id}
      role="spinbutton"
      tabIndex={0}
      // The dial's own arrows move between digits; the shell's global bindings must not also
      // fire while it is focused.
      data-hotkeys="off"
      aria-label="Tuned frequency"
      aria-disabled={disabled || undefined}
      aria-valuenow={hz}
      aria-valuemin={range.min}
      aria-valuemax={range.max}
      aria-valuetext={`${(hz / 1e6).toFixed(6)} megahertz`}
      className="flex items-baseline rounded-[3px] font-mono leading-none select-none"
      onKeyDown={disabled ? undefined : onKeyDown}
    >
      {digits.map((digit, i) => (
        <Digit
          key={digit.place}
          digit={digit}
          active={i === index}
          disabled={disabled}
          onSelect={() => setActive(i)}
          onStep={(direction) => onTune(stepDial(hz, digit.place, direction, range))}
        />
      ))}
      <span className="ml-2 self-baseline text-[11px] tracking-wide text-ink-faint">MHz</span>
    </div>
  );
}

/** Groups are drawn by the separators between digits, not by wrapper elements: a boundary that
 * is a margin cannot be clicked by mistake, and the digits stay one flat row for the arrows.
 *
 * Each digit is split across its own height (DESIGN.md §9): the upper half steps that decade up,
 * the lower half down. The armed half is tinted and takes a directional cursor *before* the
 * press, because a control that retunes the radio has to say which way it is about to go. */
function Digit({
  digit,
  active,
  disabled,
  onSelect,
  onStep,
}: {
  digit: DialDigit;
  active: boolean;
  disabled: boolean;
  onSelect: () => void;
  onStep: (direction: number) => void;
}) {
  const [armed, setArmed] = useState<number | null>(null);
  // Drawn after the digit: the point follows the megahertz place, the thin gap the
  // kilohertz group.
  const separator = digit.place === 6 ? "." : digit.place === 3 ? " " : "";
  return (
    <>
      <button
        type="button"
        tabIndex={-1}
        disabled={disabled}
        data-place={digit.place}
        aria-label={`${10 ** digit.place} hertz digit`}
        className={`relative min-h-7 overflow-hidden rounded-[2px] px-[2px] tabular-nums transition-colors duration-100 pointer-coarse:min-h-10 ${DIGIT_SIZE} ${
          armed === null ? "" : armed > 0 ? "cursor-n-resize" : "cursor-s-resize"
        } ${
          armed !== null || active ? "text-accent" : digit.leading ? "text-ink-faint" : "text-ink"
        } ${active ? "bg-accent/12 shadow-[inset_0_-2px_0_var(--color-accent)]" : ""}`}
        // The press both selects the digit for the keyboard and steps it, so a pointer user
        // never has to aim twice to move the radio one unit.
        onPointerDown={(event) => {
          onSelect();
          onStep(halfAt(event.currentTarget, event.clientY));
        }}
        // A touch pointer has no hover, so the tint only ever shows for a mouse; the press
        // itself reads the same half either way.
        onPointerMove={(event) => {
          const half = halfAt(event.currentTarget, event.clientY);
          if (half !== armed) {
            setArmed(half);
          }
        }}
        onPointerLeave={() => setArmed(null)}
        // Steps live on the digit rather than only on the group so a pointer user gets the
        // same decade-at-a-time control the keyboard has.
        onKeyDown={(event) => {
          if (event.key === "ArrowUp" || event.key === "ArrowDown") {
            event.stopPropagation();
            event.preventDefault();
            onStep(event.key === "ArrowUp" ? 1 : -1);
          }
        }}
      >
        {armed !== null && (
          <span
            aria-hidden
            className={`pointer-events-none absolute inset-x-0 h-1/2 bg-accent/18 ${
              armed > 0 ? "top-0" : "bottom-0"
            }`}
          />
        )}
        <span className="relative">{digit.digit}</span>
      </button>
      {separator !== "" && (
        <span aria-hidden className={`text-ink-dim ${DIGIT_SIZE}`}>
          {separator}
        </span>
      )}
    </>
  );
}

/** +1 in the upper half of the target, −1 in the lower. */
function halfAt(target: HTMLElement, clientY: number): number {
  const rect = target.getBoundingClientRect();
  return clientY < rect.top + rect.height / 2 ? 1 : -1;
}

function DirectEntry({
  draft,
  onDraft,
  onCommit,
}: {
  draft: string;
  onDraft: (draft: string | null) => void;
  onCommit: (hz: number) => void;
}) {
  const parsed = parseFrequency(draft);
  const empty = draft.trim() === "";
  return (
    <input
      autoFocus
      aria-label="Tune to frequency"
      placeholder="145.5 · 433800k · 2.4g"
      // Deliberately below the digits it replaces: this is a field being typed into for a
      // second, not the readout being watched, and at dial size it reads as if the instrument
      // itself had been swapped for a text box.
      className={`h-9 w-[15ch] rounded-[3px] border bg-panel-2 px-2 font-mono text-[16px] leading-none tabular-nums text-ink placeholder:text-[11px] placeholder:text-ink-faint @min-[22rem]:text-[20px] ${
        empty || parsed !== null ? "border-accent" : "border-danger"
      }`}
      value={draft}
      onChange={(event) => onDraft(event.target.value)}
      onBlur={() => onDraft(null)}
      onKeyDown={(event) => {
        if (event.key === "Enter" && parsed !== null) {
          onCommit(parsed);
        } else if (event.key === "Escape") {
          onDraft(null);
        }
      }}
    />
  );
}
