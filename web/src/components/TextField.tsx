import { useState } from "react";
import { Input } from "./BaseControls";
import { FIELD } from "./controls";

export function TextField({
  label,
  value,
  secret = false,
  placeholder,
  onCommit,
}: {
  label: string;
  value: string;
  secret?: boolean;
  placeholder?: string;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    setDraft(value);
  }
  const commit = () => {
    const next = draft.trim();
    setDraft(next);
    if (next !== value) {
      onCommit(next);
    }
  };
  return (
    <Input
      className={FIELD}
      aria-label={label}
      type={secret ? "password" : "text"}
      autoComplete="off"
      placeholder={placeholder}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.currentTarget.blur();
        }
      }}
    />
  );
}
