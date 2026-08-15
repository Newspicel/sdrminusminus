import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { Button, Input } from "./BaseControls";
import {
  type DialDigit,
  dialDigits,
  dialPlaces,
  parseFrequency,
  type Range,
  setDialDigit,
  stepDial,
} from "./dial";

export const DIAL_ID = "frequency-dial";

const DIGIT_SIZE =
  "text-[16px] @min-[17rem]:text-[20px] @min-[22rem]:text-[26px] @min-[28rem]:text-[34px]";

export function FrequencyDial({
  hz,
  range,
  onTune,
  disabled = false,
  wheelTunes = true,
  id = DIAL_ID,
}: {
  hz: number;
  range: Range;
  onTune: (hz: number) => void;
  disabled?: boolean;
  wheelTunes?: boolean;
  id?: string;
}) {
  const places = useMemo(() => dialPlaces(range.max), [range.max]);
  const digits = dialDigits(hz, places);
  const [active, setActive] = useState(() => places.indexOf(6));
  const [draft, setDraft] = useState<string | null>(null);
  const dialRef = useRef<HTMLDivElement>(null);

  const index = Math.min(active, places.length - 1);
  const place = places[index] ?? 6;

  useEffect(() => {
    const dial = dialRef.current;
    if (dial === null || draft !== null || disabled || !wheelTunes) {
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
  }, [hz, places, range, onTune, draft, disabled, wheelTunes]);

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
      id={id}
      role="spinbutton"
      tabIndex={0}
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
  const separator = digit.place === 6 ? "." : digit.place === 3 ? " " : "";
  return (
    <>
      <Button
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
        onPointerDown={(event) => {
          onSelect();
          onStep(halfAt(event.currentTarget, event.clientY));
        }}
        onPointerMove={(event) => {
          const half = halfAt(event.currentTarget, event.clientY);
          if (half !== armed) {
            setArmed(half);
          }
        }}
        onPointerLeave={() => setArmed(null)}
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
      </Button>
      {separator !== "" && (
        <span aria-hidden className={`text-ink-dim ${DIGIT_SIZE}`}>
          {separator}
        </span>
      )}
    </>
  );
}

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
    <Input
      autoFocus
      aria-label="Tune to frequency"
      placeholder="145.5 · 433800k · 2.4g"
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
