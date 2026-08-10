// The tuning dial (DESIGN.md §6) — the signature control of the UI. Every digit is its own
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

export function FrequencyDial({
  hz,
  range,
  onTune,
}: {
  hz: number;
  range: Range;
  onTune: (hz: number) => void;
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
    if (dial === null || draft !== null) {
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
  }, [hz, places, range, onTune, draft]);

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
      event.preventDefault();
      setDraft("");
    }
  };

  return (
    // biome-ignore lint/a11y/useSemanticElements: a spinbutton over ten place-value targets is
    // the roving-tabindex pattern; a native input cannot address one decade at a time.
    <div
      ref={dialRef}
      id={DIAL_ID}
      role="spinbutton"
      tabIndex={0}
      // The dial's own arrows move between digits; the shell's global bindings must not also
      // fire while it is focused.
      data-hotkeys="off"
      aria-label="Tuned frequency"
      aria-valuenow={hz}
      aria-valuemin={range.min}
      aria-valuemax={range.max}
      aria-valuetext={`${(hz / 1e6).toFixed(6)} megahertz`}
      className="flex items-baseline rounded-[3px] font-mono leading-none select-none"
      onKeyDown={onKeyDown}
      onDoubleClick={() => setDraft("")}
    >
      {digits.map((digit, i) => (
        <Digit
          key={digit.place}
          digit={digit}
          active={i === index}
          onSelect={() => setActive(i)}
          onStep={(direction) => onTune(stepDial(hz, digit.place, direction, range))}
        />
      ))}
      <span className="ml-2 self-baseline text-[11px] tracking-wide text-ink-faint">MHz</span>
    </div>
  );
}

/** Groups are drawn by the separators between digits, not by wrapper elements: a boundary that
 * is a margin cannot be clicked by mistake, and the digits stay one flat row for the arrows. */
function Digit({
  digit,
  active,
  onSelect,
  onStep,
}: {
  digit: DialDigit;
  active: boolean;
  onSelect: () => void;
  onStep: (direction: number) => void;
}) {
  // Drawn after the digit: the point follows the megahertz place, the thin gap the
  // kilohertz group.
  const separator = digit.place === 6 ? "." : digit.place === 3 ? " " : "";
  return (
    <>
      <button
        type="button"
        tabIndex={-1}
        data-place={digit.place}
        aria-label={`${10 ** digit.place} hertz digit`}
        className={`rounded-[2px] px-[2px] text-[26px] tabular-nums transition-colors duration-100 md:text-[34px] ${
          digit.leading ? "text-ink-faint" : "text-ink"
        } ${
          active ? "bg-accent/12 shadow-[inset_0_-2px_0_var(--color-accent)]" : "hover:bg-ink/8"
        }`}
        onPointerDown={onSelect}
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
        {digit.digit}
      </button>
      {separator !== "" && (
        <span aria-hidden className="text-[26px] text-ink-dim md:text-[34px]">
          {separator}
        </span>
      )}
    </>
  );
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
      className={`h-10 w-[13ch] rounded-[3px] border bg-panel-2 px-2 font-mono text-[26px] leading-none tabular-nums text-ink placeholder:text-[13px] placeholder:text-ink-faint md:h-12 md:w-[15ch] md:text-[34px] ${
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
