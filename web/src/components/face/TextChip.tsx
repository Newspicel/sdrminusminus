import { TextField } from "../TextField";
import { ChipField, SettingChip } from "./Chips";

const SHOWN_MAX = 22;

export function chipText(value: string, secret: boolean, placeholder = "none"): string {
  if (value === "") {
    return placeholder;
  }
  if (secret) {
    return "set";
  }
  return value.length > SHOWN_MAX ? `${value.slice(0, SHOWN_MAX - 1)}…` : value;
}

export function TextChip({
  label,
  title,
  value,
  name,
  shown,
  placeholder,
  secret = false,
  disabled,
  onCommit,
}: {
  label: string;
  title: string;
  value: string;
  name?: string;
  shown?: string;
  placeholder?: string;
  secret?: boolean;
  disabled?: boolean;
  onCommit: (value: string) => boolean | void;
}) {
  return (
    <SettingChip
      label={label}
      value={shown ?? chipText(value, secret, placeholder)}
      title={title}
      quiet={value === ""}
      disabled={disabled}
    >
      {() => (
        <ChipField label={name ?? title}>
          <TextField
            label={name ?? title}
            value={value}
            secret={secret}
            placeholder={placeholder}
            onCommit={(next) => {
              onCommit(next);
            }}
          />
        </ChipField>
      )}
    </SettingChip>
  );
}
