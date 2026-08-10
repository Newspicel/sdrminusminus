// Opening a device (PLAN §10, M5). One surface asks: an unbound device node, which *is* the
// invitation rather than a panel with an empty state (CANVAS §3). The discovery list, its
// ranking and the diagnostics behind it live here because what a choice *means* is the node's
// business, not the list's.
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { devicesQuery, doctorQuery } from "../lib/api";
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

/** The discovered devices, one button each, with the states discovery itself can be in. The
 * caller decides what choosing one does. */
export function DeviceChoices({
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
            // The first device is the intended next action; the rest are alternatives.
            className={`${index === 0 ? BTN_PRIMARY : BTN} justify-center`}
            disabled={busy}
            onClick={() => onChoose(device)}
          >
            <span className="truncate">{device.label}</span>
          </button>
        ))}
      </div>

      {devices.isPending && <p className="text-sm text-ink-dim">Looking for devices…</p>}
      {!devices.isPending && found.length === 0 && (
        <p className="text-sm text-ink-dim">
          No devices found. Plug one in, or check the diagnostics below.
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
