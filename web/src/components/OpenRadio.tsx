// Opening a receiver (PLAN §10, M5). Two surfaces ask the same question — the spectrum's empty
// state, and an unbound device node, which *is* the invitation rather than a panel with an empty
// state (CANVAS §3) — so the discovery list, its ranking and the diagnostics behind it are one
// component, and only what a choice *means* differs between them.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { createDeviceSet, devicesQuery, doctorQuery, STATE_KEY } from "../lib/api";
import type { DeviceInfo } from "../lib/types";
import { BTN, BTN_PRIMARY, BTN_QUIET } from "./controls";

function deviceRank(device: DeviceInfo): number {
  return device.driver === "virtual" ? 1 : 0;
}

/** Hardware first, then the virtual devices — someone with a dongle attached should not have to
 * read past the signal generator to find it. */
export function rankDevices(devices: readonly DeviceInfo[]): readonly DeviceInfo[] {
  return devices.toSorted(
    (a, b) => deviceRank(a) - deviceRank(b) || a.label.localeCompare(b.label),
  );
}

/** The id `POST /api/devicesets` opens a device by. Not a `DeviceRef`: that names a radio in a
 * stored patch, this addresses one probe result in this run (CANVAS §3). */
export function deviceId(device: DeviceInfo): string {
  return `${device.driver}:${device.key}`;
}

/** The discovered receivers, one button each, with the states discovery itself can be in. The
 * caller decides what choosing one does. */
export function ReceiverChoices({
  onChoose,
  busy = false,
  error = null,
}: {
  onChoose: (device: DeviceInfo) => void;
  busy?: boolean;
  error?: string | null;
}) {
  const devices = useQuery(devicesQuery());
  const [showDoctor, setShowDoctor] = useState(false);
  const found = rankDevices(devices.data?.devices ?? []);

  return (
    <div className="flex w-full flex-col gap-2">
      <div className="flex flex-col gap-1">
        {found.map((device, index) => (
          <button
            key={deviceId(device)}
            type="button"
            // The first receiver is the intended next action; the rest are alternatives.
            className={`${index === 0 ? BTN_PRIMARY : BTN} justify-center`}
            disabled={busy}
            onClick={() => onChoose(device)}
          >
            <span className="truncate">{device.label}</span>
          </button>
        ))}
      </div>

      {devices.isPending && <p className="text-sm text-ink-dim">Looking for receivers…</p>}
      {!devices.isPending && found.length === 0 && (
        <p className="text-sm text-ink-dim">
          No receivers found. Plug one in, or check the diagnostics below.
        </p>
      )}

      {error !== null && (
        <p role="alert" className="font-mono text-xs text-danger">
          {error}
        </p>
      )}

      <button
        type="button"
        className={`${BTN_QUIET} self-center`}
        onClick={() => setShowDoctor(!showDoctor)}
      >
        {showDoctor ? "Hide diagnostics" : "Hardware not showing up?"}
      </button>
      {showDoctor && <Doctor />}
    </div>
  );
}

/** The spectrum's empty state: this was a first-run banner that took a row of chrome from every
 * session, and it is now where someone with no radio open is already looking. */
export function OpenRadio({ onOpened }: { onOpened: (ds: number) => void }) {
  const queryClient = useQueryClient();
  const openMut = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: onOpened,
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  return (
    <div className="flex w-full max-w-md flex-col items-center gap-4 text-center">
      <div className="flex flex-col gap-1">
        <h2 className="text-base font-medium text-ink">Open a receiver</h2>
        <p className="text-sm text-ink-dim">
          No hardware? The signal generator and any recording play back exactly like a device.
        </p>
      </div>
      <ReceiverChoices
        onChoose={(device) => openMut.mutate(deviceId(device))}
        busy={openMut.isPending}
        error={openMut.error?.message ?? null}
      />
    </div>
  );
}

/** The `--doctor` report, rendered where a stuck first-time user will actually look for it. */
function Doctor() {
  const doctor = useQuery(doctorQuery(true));
  if (doctor.isPending) {
    return <p className="text-sm text-ink-dim">Checking…</p>;
  }
  if (doctor.error) {
    return (
      <p role="alert" className="font-mono text-xs text-danger">
        Diagnostics failed: {doctor.error.message}
      </p>
    );
  }
  return (
    <dl className="flex w-full flex-col gap-2 text-left">
      {(doctor.data?.checks ?? []).map((check) => (
        <div key={check.id}>
          <dt className="flex items-center gap-2 font-mono text-xs">
            <span
              className={
                check.status === "fail"
                  ? "text-danger"
                  : check.status === "warn"
                    ? "text-ink"
                    : "text-ok"
              }
            >
              [{check.status}]
            </span>
            <span className="text-ink">{check.name}</span>
          </dt>
          <dd className="legend pl-4 whitespace-pre-wrap normal-case">
            {check.detail}
            {check.hint != null && `\n→ ${check.hint}`}
          </dd>
        </div>
      ))}
    </dl>
  );
}
