import { BTN } from "./controls";
import { Popover } from "./Popover";

export interface DownloadChoice {
  label: string;
  href: string;
  hint?: string;
}

export function DownloadMenu({
  choices,
  label = "Download",
  disabled = false,
  triggerClass = BTN,
}: {
  choices: readonly DownloadChoice[];
  label?: string;
  disabled?: boolean;
  triggerClass?: string;
}) {
  return (
    <Popover
      label={label}
      triggerClass={`${triggerClass} shrink-0`}
      width="w-56"
      align="end"
      padded={false}
      disabled={disabled}
    >
      {(close) => (
        <div className="flex flex-col p-1">
          {choices.map((choice) => (
            <a
              key={choice.label}
              className={`${BTN} justify-start border-transparent bg-transparent`}
              href={choice.href}
              title={choice.hint}
              download
              onClick={close}
            >
              {choice.label}
            </a>
          ))}
        </div>
      )}
    </Popover>
  );
}
