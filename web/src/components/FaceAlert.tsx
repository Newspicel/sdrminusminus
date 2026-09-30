import { X } from "lucide-react";
import { useEffect, useState } from "react";
import { type Refusal, useRefusalStore, visibleRefusal, wireShownUntil } from "../lib/refusals";
import { Button } from "./BaseControls";
import { ICON_BTN_SM } from "./controls";
import { Icon } from "./Icon";

const NO_REFUSALS: readonly Refusal[] = [];

function useNowAfter(until: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (until === null) {
      return;
    }
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, until - Date.now() + 1));
    return () => clearTimeout(timer);
  }, [until]);
  return now;
}

export function FaceAlert({ node }: { node: string }) {
  const list = useRefusalStore((store) => store.byNode[node] ?? NO_REFUSALS);
  const dismiss = useRefusalStore((store) => store.dismiss);
  const now = useNowAfter(wireShownUntil(list));
  const shown = visibleRefusal(list, now);
  if (shown === null) {
    return null;
  }
  return (
    <div
      role="alert"
      className="flex shrink-0 items-center gap-1 border-b border-danger/40 bg-danger/10 py-0.5 pr-1 pl-2 text-xs text-danger"
    >
      <span className="min-w-0 flex-1 truncate" title={shown.reason}>
        {shown.reason}
      </span>
      <Button
        type="button"
        aria-label="Dismiss"
        title="Dismiss"
        className={`${ICON_BTN_SM} text-danger`}
        onClick={() => dismiss(node)}
      >
        <Icon glyph={X} size={12} />
      </Button>
    </div>
  );
}
