import type { ReactNode } from "react";
import { Button } from "./BaseControls";
import { CHIP_READOUT, CHIP_SETTING } from "./controls";
import { Popover } from "./Popover";
import { Tip } from "./Tip";

function ChipFace({
  label,
  value,
  unit,
  quiet,
}: {
  label: string;
  value: string;
  unit?: string;
  quiet: boolean;
}) {
  return (
    <>
      <span className="font-sans">{label}</span>
      <b className={quiet ? "font-normal text-ink-dim" : "font-medium text-ink"}>{value}</b>
      {unit !== undefined && unit !== "" && <span>{unit}</span>}
    </>
  );
}

export function SettingChip({
  label,
  value,
  unit,
  title,
  quiet = false,
  width = "w-64",
  children,
}: {
  label: string;
  value: string;
  unit?: string;
  title: string;
  quiet?: boolean;
  width?: string;
  children: (close: () => void) => ReactNode;
}) {
  return (
    <Popover
      label={<ChipFace label={label} value={value} unit={unit} quiet={quiet} />}
      title={title}
      triggerClass={CHIP_SETTING}
      width={width}
    >
      {children}
    </Popover>
  );
}

export function ReadoutChip({
  label,
  value,
  unit,
  title,
}: {
  label: string;
  value: string;
  unit?: string;
  title: string;
}) {
  return (
    <span className={CHIP_READOUT} title={title}>
      <ChipFace label={label} value={value} unit={unit} quiet={false} />
    </span>
  );
}

export function ToggleChip({
  label,
  on,
  title,
  onChange,
}: {
  label: string;
  on: boolean;
  title: string;
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
          onClick={() => onChange(!on)}
        />
      }
    >
      <span className="font-sans">{label}</span>
      <b className={on ? "font-medium text-accent" : "font-normal text-ink-dim"}>
        {on ? "on" : "off"}
      </b>
    </Tip>
  );
}
