import { Autocomplete as Primitive } from "@base-ui/react/autocomplete";
import { useState } from "react";
import { commitText, FIELD, SURFACE } from "./controls";
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
    setDraft(commitText(candidate, value, onCommit));
  };

  return (
    <Primitive.Root
      items={suggestions}
      value={draft}
      itemToStringValue={(suggestion) => suggestion.value}
      mode="none"
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
            <Primitive.Empty className="text-xs text-ink-dim">
              <span className="block px-2 py-1.5">
                No detected match. A custom value is still accepted.
              </span>
            </Primitive.Empty>
            <Primitive.List className="flex max-h-[var(--available-height)] flex-col overflow-y-auto p-0.5">
              {(suggestion: AutocompleteSuggestion) => (
                <Primitive.Item
                  value={suggestion}
                  className={(state) =>
                    "flex cursor-default flex-col items-start rounded-[3px] px-2 py-1 text-xs " +
                    "leading-tight " +
                    (suggestion.value === value
                      ? "bg-accent/15 text-accent"
                      : state.highlighted
                        ? "bg-panel-2 text-ink"
                        : "text-ink")
                  }
                >
                  <span className="font-mono">{suggestion.value}</span>
                  {suggestion.detail !== undefined && (
                    <span className="opacity-70">{suggestion.detail}</span>
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
