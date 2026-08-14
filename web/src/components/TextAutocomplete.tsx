import { Autocomplete as Primitive } from "@base-ui/react/autocomplete";
import { useState } from "react";
import { FIELD, SURFACE } from "./controls";
import { usePortalContainer } from "./PortalContainer";

export interface AutocompleteSuggestion {
  value: string;
  detail?: string;
}

export function TextAutocomplete({
  label,
  value,
  suggestions,
  placeholder,
  inputMode,
  className,
  onCommit,
}: {
  label: string;
  value: string;
  suggestions: readonly AutocompleteSuggestion[];
  placeholder?: string;
  inputMode?: "decimal" | "numeric" | "search" | "text";
  className?: string;
  onCommit: (value: string) => boolean;
}) {
  const [draft, setDraft] = useDraft(value);
  const portalContainer = usePortalContainer();

  const commit = (candidate: string): void => {
    const next = candidate.trim();
    if (next === "" || (next !== value && !onCommit(next))) {
      setDraft(value);
    } else {
      setDraft(next);
    }
  };

  return (
    <Primitive.Root
      items={suggestions}
      value={draft}
      itemToStringValue={(suggestion) => suggestion.value}
      openOnInputClick
      onValueChange={(next, details) => {
        setDraft(next);
        if (details.reason === "item-press") {
          commit(next);
        }
      }}
    >
      <Primitive.InputGroup className={`relative min-w-0 ${className ?? ""}`}>
        <Primitive.Input
          data-hotkeys="off"
          aria-label={label}
          inputMode={inputMode}
          placeholder={placeholder}
          className={`${FIELD} w-full pr-7`}
          onBlur={(event) => commit(event.currentTarget.value)}
        />
        <Primitive.Trigger
          data-hotkeys="off"
          aria-label={`Show ${label.toLowerCase()} suggestions`}
          className="absolute inset-y-0 right-0 flex w-7 cursor-pointer items-center justify-center text-ink-faint hover:text-ink"
        >
          ▾
        </Primitive.Trigger>
      </Primitive.InputGroup>
      <Primitive.Portal container={portalContainer} className="contents">
        <Primitive.Positioner className="z-30" side="bottom" align="start" sideOffset={4}>
          <Primitive.Popup
            data-hotkeys="off"
            className={`${SURFACE} max-w-[calc(100vw-1rem)] min-w-[var(--anchor-width)]`}
          >
            <Primitive.Empty className="px-2 py-1.5 text-xs text-ink-dim">
              No detected match. A custom value is still accepted.
            </Primitive.Empty>
            <Primitive.List className="flex max-h-[var(--available-height)] flex-col overflow-y-auto p-0.5">
              {(suggestion: AutocompleteSuggestion) => (
                <Primitive.Item
                  value={suggestion}
                  className="flex cursor-default flex-col items-start rounded-[3px] px-2 py-1 text-xs leading-tight text-ink-dim data-highlighted:bg-panel-2 data-highlighted:text-ink"
                >
                  <span className="font-mono text-ink">{suggestion.value}</span>
                  {suggestion.detail !== undefined && (
                    <span className="text-ink-dim">{suggestion.detail}</span>
                  )}
                </Primitive.Item>
              )}
            </Primitive.List>
          </Primitive.Popup>
        </Primitive.Positioner>
      </Primitive.Portal>
    </Primitive.Root>
  );
}

function useDraft(value: string): [string, (value: string) => void] {
  const [draft, setDraft] = useState(value);
  const [seen, setSeen] = useState(value);
  if (seen !== value) {
    setSeen(value);
    setDraft(value);
    return [value, setDraft];
  }
  return [draft, setDraft];
}
