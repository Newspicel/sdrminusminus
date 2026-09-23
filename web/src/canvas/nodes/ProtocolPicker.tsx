import { ChevronDown } from "lucide-react";
import { Checkbox } from "../../components/Checkbox";
import { BTN_SM } from "../../components/controls";
import { Icon } from "../../components/Icon";
import { Popover } from "../../components/Popover";
import {
  activePreset,
  choiceSummary,
  enabledKinds,
  type Protocol,
  type ProtocolChoice,
  type ProtocolGroup,
  protocolPresets,
  setEnabled,
} from "./protocols";

const TRIGGER =
  "relative inline-flex h-7 w-full max-w-52 items-center gap-1.5 overflow-hidden rounded-[3px] border border-line " +
  "bg-well px-2 text-xs text-ink hover:border-line-strong focus-visible:border-accent-dim " +
  "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/20 " +
  "data-popup-open:border-accent-dim";

const KEY =
  "inline-flex h-6 items-center gap-1.5 rounded-[3px] border px-1.5 font-mono text-[11px] " +
  "transition-colors duration-100 focus-visible:outline-none focus-visible:ring-2 " +
  "focus-visible:ring-accent/30";

const KEY_ON = "border-accent-dim bg-accent/10 text-ink";

const KEY_OFF = "border-line bg-well text-ink-faint hover:border-line-strong hover:text-ink-dim";

export function ProtocolPicker({
  groups,
  choice,
  onChange,
}: {
  groups: readonly ProtocolGroup[];
  choice: ProtocolChoice;
  onChange: (choice: ProtocolChoice) => void;
}) {
  return (
    <Popover
      label={<Trigger groups={groups} choice={choice} />}
      title="Protocols to decode"
      triggerClass={TRIGGER}
      width="w-84"
      padded={false}
    >
      {() => <Panel groups={groups} choice={choice} onChange={onChange} />}
    </Popover>
  );
}

function Trigger({ groups, choice }: { groups: readonly ProtocolGroup[]; choice: ProtocolChoice }) {
  return (
    <>
      <span className="min-w-0 flex-1 truncate pb-0.5 text-left">
        {choiceSummary(groups, choice)}
      </span>
      <Icon glyph={ChevronDown} size={12} />
      <BankLights groups={groups} disabled={choice.disabled} />
    </>
  );
}

function BankLights({
  groups,
  disabled,
}: {
  groups: readonly ProtocolGroup[];
  disabled: readonly string[];
}) {
  return (
    <span aria-hidden className="absolute inset-x-1.5 bottom-[3px] flex gap-1">
      {groups.map((group) => (
        <span
          key={group.family}
          className="h-[2px] flex-1 overflow-hidden rounded-full bg-line-strong/40"
        >
          <span
            className="block h-full rounded-full bg-accent shadow-[0_0_3px_var(--color-vfo-glow)]"
            style={{
              width: `${(enabledKinds([group], disabled).length / group.protocols.length) * 100}%`,
            }}
          />
        </span>
      ))}
    </span>
  );
}

function Panel({
  groups,
  choice,
  onChange,
}: {
  groups: readonly ProtocolGroup[];
  choice: ProtocolChoice;
  onChange: (choice: ProtocolChoice) => void;
}) {
  const setDisabled = (kinds: readonly string[], enabled: boolean) =>
    onChange({ ...choice, disabled: setEnabled(choice.disabled, kinds, enabled) });
  return (
    <div className="flex flex-col">
      <Presets groups={groups} choice={choice} onChange={onChange} />
      <ul className="flex flex-col divide-y divide-line">
        {groups.map((group) => (
          <Family
            key={group.family}
            group={group}
            disabled={choice.disabled}
            onToggle={setDisabled}
          />
        ))}
      </ul>
      <label className="flex cursor-pointer items-center gap-2 border-t border-line bg-panel-2/40 px-3 py-2 text-xs text-ink-dim">
        <Checkbox
          label="Unidentified signals"
          checked={choice.unidentified}
          onChange={(unidentified) => onChange({ ...choice, unidentified })}
        />
        Unidentified signals
      </label>
    </div>
  );
}

function Presets({
  groups,
  choice,
  onChange,
}: {
  groups: readonly ProtocolGroup[];
  choice: ProtocolChoice;
  onChange: (choice: ProtocolChoice) => void;
}) {
  const active = activePreset(groups, choice)?.id;
  return (
    <div className="flex flex-wrap gap-1 border-b border-line p-2">
      {protocolPresets(groups).map((preset) => (
        <button
          key={preset.id}
          type="button"
          aria-pressed={preset.id === active}
          className={`${BTN_SM} aria-pressed:border-accent aria-pressed:bg-accent aria-pressed:text-on-accent`}
          onClick={() => onChange(preset.choice)}
        >
          {preset.label}
        </button>
      ))}
    </div>
  );
}

function Family({
  group,
  disabled,
  onToggle,
}: {
  group: ProtocolGroup;
  disabled: readonly string[];
  onToggle: (kinds: readonly string[], enabled: boolean) => void;
}) {
  const kinds = group.protocols.map((protocol) => protocol.kind);
  const on = enabledKinds([group], disabled).length;
  return (
    <li className="flex flex-col gap-1.5 px-3 py-2">
      <label className="flex cursor-pointer items-center gap-2 text-xs text-ink">
        <Checkbox
          label={group.title}
          checked={on === kinds.length}
          onChange={(enabled) => onToggle(kinds, enabled)}
        />
        <span className="flex-1">{group.title}</span>
        <span className="font-mono text-[10.5px] tabular-nums text-ink-faint">
          {on}/{kinds.length}
        </span>
      </label>
      <div className="flex flex-wrap gap-1 pl-6">
        {group.protocols.map((protocol) => (
          <Key
            key={protocol.kind}
            protocol={protocol}
            on={!disabled.includes(protocol.kind)}
            onToggle={(enabled) => onToggle([protocol.kind], enabled)}
          />
        ))}
      </div>
    </li>
  );
}

function Key({
  protocol,
  on,
  onToggle,
}: {
  protocol: Protocol;
  on: boolean;
  onToggle: (enabled: boolean) => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={on}
      title={protocol.name}
      className={`${KEY} ${on ? KEY_ON : KEY_OFF}`}
      onClick={() => onToggle(!on)}
    >
      <span
        aria-hidden
        className={`size-1.5 rounded-full ${
          on ? "bg-accent shadow-[0_0_4px_var(--color-accent)]" : "bg-line-strong"
        }`}
      />
      {protocol.label}
    </button>
  );
}
