import { useState } from "react";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
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
    <Combobox<AutocompleteSuggestion>
      items={suggestions}
      inputValue={draft}
      itemToStringValue={(suggestion) => suggestion.value}
      openOnInputClick
      onInputValueChange={setDraft}
      onValueChange={(next) => {
        if (next !== null) {
          setDraft(next.value);
          commit(next.value);
        }
      }}
    >
      <ComboboxInput
        data-hotkeys="off"
        aria-label={label}
        inputMode={inputMode}
        placeholder={placeholder}
        className={className}
        onBlur={(event) => commit(event.currentTarget.value)}
      />
      <ComboboxContent container={portalContainer} data-hotkeys="off">
        <ComboboxEmpty>No detected match. A custom value is still accepted.</ComboboxEmpty>
        <ComboboxList>
          {(suggestion: AutocompleteSuggestion) => (
            <ComboboxItem value={suggestion} className="flex-col items-start text-xs leading-tight">
              <span className="font-mono">{suggestion.value}</span>
              {suggestion.detail !== undefined && (
                <span className="text-muted-foreground">{suggestion.detail}</span>
              )}
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
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
