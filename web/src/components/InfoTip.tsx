import { Info } from "lucide-react";
import { Button } from "./BaseControls";
import { Icon } from "./Icon";
import { Tip } from "./Tip";

export function InfoTip({ text }: { text: string }) {
  return (
    <Tip
      text={text}
      render={
        <Button
          type="button"
          aria-label={text}
          tabIndex={-1}
          className="inline-flex size-3.5 shrink-0 items-center justify-center rounded-full text-ink-faint/70 hover:text-accent"
        />
      }
    >
      <Icon glyph={Info} size={12} />
    </Tip>
  );
}
