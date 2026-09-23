import { Popover } from "@base-ui/react/popover";
import { Info } from "lucide-react";
import { SURFACE } from "./controls";
import { Icon } from "./Icon";
import { usePortalContainer } from "./PortalContainer";

const HOVER_DELAY_MS = 250;

export function InfoTip({ text }: { text: string }) {
  const portalContainer = usePortalContainer();
  return (
    <Popover.Root>
      <Popover.Trigger
        openOnHover
        delay={HOVER_DELAY_MS}
        aria-label={text}
        className="nodrag nopan inline-flex size-3.5 shrink-0 cursor-help items-center justify-center rounded-full text-ink-faint/70 hover:text-accent data-popup-open:text-accent"
      >
        <Icon glyph={Info} size={12} />
      </Popover.Trigger>
      <Popover.Portal container={portalContainer} className="contents">
        <Popover.Positioner
          className="z-30 nodrag nopan"
          side="top"
          sideOffset={6}
          collisionPadding={8}
        >
          <Popover.Popup className={`${SURFACE} max-w-72 px-2 py-1 text-xs text-balance`}>
            {text}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
