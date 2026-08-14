import { Dialog } from "@base-ui/react/dialog";
import { BINDINGS } from "../canvas/useHotkeys";
import { BTN, SURFACE } from "./controls";

export function Shortcuts({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 w-full max-w-md -translate-x-1/2 -translate-y-1/2 p-4`}
        >
          <div className="flex items-baseline justify-between gap-4">
            <Dialog.Title className="text-base font-medium text-ink">Keyboard</Dialog.Title>
            <Dialog.Description className="legend">
              Ignored while a field has focus
            </Dialog.Description>
          </div>
          <dl className="mt-3 grid grid-cols-[8rem_1fr] gap-x-4 gap-y-1.5">
            {BINDINGS.map((binding) => (
              <div key={binding.keys} className="contents">
                <dt className="text-right font-mono text-xs text-accent">{binding.keys}</dt>
                <dd className="text-xs text-ink-dim">{binding.what}</dd>
              </div>
            ))}
          </dl>
          <div className="mt-4 flex justify-end">
            <Dialog.Close className={BTN}>Close</Dialog.Close>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
