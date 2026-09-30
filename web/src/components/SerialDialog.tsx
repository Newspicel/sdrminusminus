import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { ApiRequestError, devicesQuery, writeSerial } from "../lib/api";
import { askedDevices, markAsked, nextToAsk, promptOff, turnPromptOff } from "../lib/serialPrompt";
import type { DeviceInfo } from "../lib/types";
import { Button, Input } from "./BaseControls";
import { Checkbox } from "./Checkbox";
import { BTN_PRIMARY, BTN_QUIET, DIALOG_TITLE, FIELD, SURFACE } from "./controls";
import { deviceId } from "./devices";

function failure(error: unknown): string {
  if (error instanceof ApiRequestError && error.status === 409) {
    return "Close this radio on its Device node first.";
  }
  return error instanceof Error ? error.message : String(error);
}

export function SerialDialog() {
  const devices = useQuery(devicesQuery());
  const [asked, setAsked] = useState(askedDevices);
  const [off, setOff] = useState(promptOff);
  const device = off ? undefined : nextToAsk(devices.data?.devices ?? [], asked);
  if (device === undefined) {
    return null;
  }
  const dismiss = (neverAgain: boolean) => {
    if (neverAgain) {
      turnPromptOff();
      setOff(true);
    }
    setAsked(markAsked(deviceId(device)));
  };
  return <SerialPrompt key={deviceId(device)} device={device} onDismiss={dismiss} />;
}

function SerialPrompt({
  device,
  onDismiss,
}: {
  device: DeviceInfo;
  onDismiss: (neverAgain: boolean) => void;
}) {
  const [serial, setSerial] = useState("");
  const [neverAgain, setNeverAgain] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [written, setWritten] = useState<string | null>(null);

  const write = async () => {
    setBusy(true);
    setError(null);
    try {
      setWritten(await writeSerial(deviceId(device), serial.trim()));
    } catch (caught) {
      setError(failure(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onDismiss(neverAgain);
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 w-full max-w-sm -translate-x-1/2 -translate-y-1/2 p-4`}
        >
          {written === null ? (
            <>
              <Dialog.Title className={DIALOG_TITLE}>Give this RTL-SDR a serial?</Dialog.Title>
              <Dialog.Description className="mt-3 flex flex-col gap-2 text-xs text-ink-dim">
                <span>
                  {device.label} has no serial of its own, so it is told apart by its USB port.
                </span>
                <span>
                  With one, its settings and calibration stay with it on any port and next to other
                  dongles.
                </span>
              </Dialog.Description>
              <label className="mt-3 flex items-center gap-2 text-xs text-ink-dim">
                Serial
                <Input
                  className={`${FIELD} w-32`}
                  value={serial}
                  placeholder="Auto"
                  maxLength={16}
                  title="1 to 16 letters or digits. Empty picks a random one."
                  onChange={(event) => setSerial(event.target.value)}
                />
              </label>
              {error !== null && (
                <p role="alert" className="mt-2 text-xs text-danger">
                  {error}
                </p>
              )}
              <label className="mt-3 flex items-center gap-2 text-xs text-ink-dim">
                <Checkbox checked={neverAgain} onChange={setNeverAgain} />
                Don't show again
              </label>
              <div className="mt-4 flex justify-end gap-2 border-t border-line pt-3">
                <Dialog.Close className={BTN_QUIET}>No</Dialog.Close>
                <Button
                  type="button"
                  className={BTN_PRIMARY}
                  disabled={busy}
                  onClick={() => void write()}
                >
                  Yes
                </Button>
              </div>
            </>
          ) : (
            <>
              <Dialog.Title className={DIALOG_TITLE}>Serial {written} written</Dialog.Title>
              <Dialog.Description className="mt-3 text-xs text-ink-dim">
                Replug the dongle to use it.
              </Dialog.Description>
              <div className="mt-4 flex justify-end border-t border-line pt-3">
                <Dialog.Close className={BTN_PRIMARY}>OK</Dialog.Close>
              </div>
            </>
          )}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
