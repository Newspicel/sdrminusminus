import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";
import { cancelLeave, confirmLeave, useAutoOff } from "../lib/autoOff";
import { Button } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN_PRIMARY, BTN_QUIET, DIALOG_TITLE, SURFACE } from "./controls";

export function AutoOffDialog() {
  const open = useAutoOff((state) => state.pending !== null);
  const [neverAgain, setNeverAgain] = useState(false);
  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          cancelLeave();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 w-full max-w-sm -translate-x-1/2 -translate-y-1/2 p-4`}
        >
          <Dialog.Title className={DIALOG_TITLE}>Leave Auto tuning?</Dialog.Title>
          <Dialog.Description className="mt-3 flex flex-col gap-2 text-xs text-ink-dim">
            <span>
              On Auto, the radio follows your decoders. Set a decoder's frequency and the radio
              moves to cover it.
            </span>
            <span>
              On Manual, the radio stays where you put it. Decoders outside its window stop.
            </span>
          </Dialog.Description>
          <label className="mt-3 flex items-center gap-2 text-xs text-ink-dim">
            <Checkbox checked={neverAgain} onChange={setNeverAgain} />
            Don't show again
          </label>
          <div className="mt-4 flex justify-end gap-2 border-t border-line pt-3">
            <Dialog.Close className={BTN_QUIET}>Stay on Auto</Dialog.Close>
            <Button type="button" className={BTN_PRIMARY} onClick={() => confirmLeave(neverAgain)}>
              Switch to Manual
            </Button>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
