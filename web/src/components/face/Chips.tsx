import type { ReactNode } from "react";
import { Button } from "../BaseControls";
import { CHIP_READOUT, CHIP_SETTING, LABEL, listItem, type Options } from "../controls";
import { NumberField, OptionalNumberField } from "../NumberField";
import { Popover } from "../Popover";
import { Tip } from "../Tip";

export type ChipTone = "ok" | "warn" | "danger";

const TONE: Record<ChipTone, string> = {
  ok: "text-ok",
  warn: "text-warn",
  danger: "text-danger",
};

export function Chips({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      className={`flex flex-wrap items-center gap-x-1 gap-y-[3px] contain-inline-size ${className ?? ""}`}
    >
      {children}
    </div>
  );
}

function ChipFace({
  label,
  value,
  unit,
  quiet,
  tone,
}: {
  label: string;
  value: string;
  unit?: string;
  quiet: boolean;
  tone?: ChipTone;
}) {
  const shade =
    tone !== undefined
      ? `font-medium ${TONE[tone]}`
      : quiet
        ? "font-normal text-ink-dim"
        : "font-medium text-ink";
  return (
    <>
      <span className="font-sans">{label}</span>
      <b className={shade}>{value}</b>
      {unit !== undefined && unit !== "" && <span>{unit}</span>}
    </>
  );
}

export function ChipField({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <span className={LABEL}>{label}</span>
      <span className="flex min-w-0 flex-wrap items-center gap-2">{children}</span>
    </div>
  );
}

export function SettingChip({
  label,
  value,
  unit,
  title,
  quiet = false,
  tone,
  disabled = false,
  padded = true,
  width = "w-64",
  children,
}: {
  label: string;
  value: string;
  unit?: string;
  title: string;
  quiet?: boolean;
  tone?: ChipTone;
  disabled?: boolean;
  padded?: boolean;
  width?: string;
  children: (close: () => void) => ReactNode;
}) {
  return (
    <Popover
      label={<ChipFace label={label} value={value} unit={unit} quiet={quiet} tone={tone} />}
      title={title}
      triggerClass={CHIP_SETTING}
      disabled={disabled}
      padded={padded}
      width={width}
    >
      {children}
    </Popover>
  );
}

export function ChoiceChip<T extends string | number>({
  label,
  value,
  options,
  title,
  quiet,
  tone,
  disabled,
  onChange,
}: {
  label: string;
  value: T;
  options: Options<T>;
  title: string;
  quiet?: boolean;
  tone?: ChipTone;
  disabled?: boolean;
  onChange: (value: T) => void;
}) {
  const current = options.find((option) => option.value === value);
  return (
    <SettingChip
      label={label}
      value={current?.label ?? String(value)}
      title={title}
      quiet={quiet}
      tone={tone}
      disabled={disabled}
      padded={false}
      width="w-44"
    >
      {(close) => (
        <div role="listbox" aria-label={title} className="flex flex-col py-1">
          {options.map((option) => (
            <Button
              key={String(option.value)}
              type="button"
              role="option"
              aria-selected={option.value === value}
              title={option.title}
              disabled={option.disabled}
              className={listItem(option.value === value, false)}
              onClick={() => {
                if (option.value !== value) {
                  onChange(option.value);
                }
                close();
              }}
            >
              {option.label}
            </Button>
          ))}
        </div>
      )}
    </SettingChip>
  );
}

export function NumberChip({
  label,
  value,
  shown,
  unit,
  title,
  min,
  max,
  step,
  quiet,
  disabled,
  onCommit,
}: {
  label: string;
  value: number;
  shown?: string;
  unit?: string;
  title: string;
  min?: number;
  max?: number;
  step?: number;
  quiet?: boolean;
  disabled?: boolean;
  onCommit: (value: number) => void;
}) {
  return (
    <SettingChip
      label={label}
      value={shown ?? String(value)}
      unit={shown === undefined ? unit : undefined}
      title={title}
      quiet={quiet}
      disabled={disabled}
      width="w-52"
    >
      {() => (
        <ChipField label={title}>
          <NumberField
            className="min-w-0 flex-1"
            label={title}
            value={value}
            unit={unit}
            min={min}
            max={max}
            step={step}
            onCommit={onCommit}
          />
        </ChipField>
      )}
    </SettingChip>
  );
}

export function OptionalNumberChip({
  label,
  value,
  placeholder,
  shown,
  unit,
  title,
  min,
  max,
  step,
  onCommit,
}: {
  label: string;
  value: number | null;
  placeholder: string;
  shown?: string;
  unit?: string;
  title: string;
  min?: number;
  max?: number;
  step?: number;
  onCommit: (value: number | null) => void;
}) {
  return (
    <SettingChip
      label={label}
      value={value === null ? placeholder : (shown ?? String(value))}
      unit={value === null || shown !== undefined ? undefined : unit}
      title={title}
      quiet={value === null}
      width="w-52"
    >
      {() => (
        <ChipField label={title}>
          <OptionalNumberField
            className="min-w-0 flex-1"
            label={title}
            placeholder={placeholder}
            value={value}
            unit={unit}
            min={min}
            max={max}
            step={step}
            onCommit={onCommit}
          />
        </ChipField>
      )}
    </SettingChip>
  );
}

export function ReadoutChip({
  label,
  value,
  unit,
  title,
  tone,
}: {
  label: string;
  value: string;
  unit?: string;
  title: string;
  tone?: ChipTone;
}) {
  return (
    <span className={CHIP_READOUT} title={title}>
      <ChipFace label={label} value={value} unit={unit} quiet={false} tone={tone} />
    </span>
  );
}

export function ToggleChip({
  label,
  on,
  title,
  icon,
  disabled = false,
  onChange,
}: {
  label: string;
  on: boolean;
  title: string;
  icon?: ReactNode;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <Tip
      text={title}
      render={
        <Button
          type="button"
          className={CHIP_SETTING}
          aria-label={title}
          aria-pressed={on}
          disabled={disabled}
          onClick={() => onChange(!on)}
        />
      }
    >
      {icon}
      <span className="font-sans">{label}</span>
      <b className={on ? "font-medium text-accent" : "font-normal text-ink-dim"}>
        {on ? "on" : "off"}
      </b>
    </Tip>
  );
}
