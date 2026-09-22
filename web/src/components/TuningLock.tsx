import { Lock, LockOpen } from "lucide-react";
import { Button } from "./BaseControls";
import { ICON_BTN } from "./controls";
import { Icon } from "./Icon";
import { Tip } from "./Tip";

export function TuningLock({
  locked,
  held,
  free,
  hold = null,
  onLock,
}: {
  locked: boolean;
  held: string;
  free: string;
  hold?: string | null;
  onLock: (locked: boolean) => void;
}) {
  if (hold !== null) {
    return <HeldLock reason={hold} />;
  }
  return (
    <Tip
      text={locked ? held : free}
      render={
        <Button
          type="button"
          className={`${ICON_BTN} ${locked ? "bg-accent/15" : ""}`}
          aria-label={locked ? "Unlock tuning" : "Lock tuning"}
          aria-pressed={locked}
          onClick={() => onLock(!locked)}
        />
      }
    >
      <span className={locked ? "flex text-accent" : "flex"}>
        <Icon glyph={locked ? Lock : LockOpen} size={16} />
      </span>
    </Tip>
  );
}

export function HeldLock({ reason }: { reason: string }) {
  return (
    <Tip
      text={reason}
      render={
        <Button
          type="button"
          className={`${ICON_BTN} cursor-default bg-accent/15 hover:bg-accent/15`}
          aria-label={reason}
          disabled
          focusableWhenDisabled
        />
      }
    >
      <span className="flex text-accent">
        <Icon glyph={Lock} size={16} />
      </span>
    </Tip>
  );
}
