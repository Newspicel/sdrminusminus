import { Lock, LockOpen } from "lucide-react";
import { Button } from "./BaseControls";
import { ICON_BTN } from "./controls";
import { Icon } from "./Icon";

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
    <Button
      type="button"
      className={`${ICON_BTN} ${locked ? "bg-accent/15" : ""}`}
      aria-label={locked ? "Unlock tuning" : "Lock tuning"}
      aria-pressed={locked}
      title={locked ? held : free}
      onClick={() => onLock(!locked)}
    >
      <span className={locked ? "flex text-accent" : "flex"}>
        <Icon glyph={locked ? Lock : LockOpen} size={16} />
      </span>
    </Button>
  );
}
