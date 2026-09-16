import { Lock, LockOpen } from "lucide-react";
import { Button } from "./BaseControls";
import { ICON_BTN } from "./controls";
import { Icon } from "./Icon";
import { Tip } from "./Tip";

export function TuningLock({
  locked,
  held,
  free,
  onLock,
}: {
  locked: boolean;
  held: string;
  free: string;
  onLock: (locked: boolean) => void;
}) {
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
