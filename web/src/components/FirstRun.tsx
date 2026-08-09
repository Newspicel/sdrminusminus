// First-run wizard (PLAN §10, M5): detect hardware, open it, pick a template. Shown when the
// server has never been configured and the user has not dismissed it.
//
// "First run" is deliberately two facts: the *server* is untouched (no device sets, no presets
// — server state, so a second browser sees the same thing) and *this browser* has not
// dismissed the wizard (a UI preference, which is what localStorage is for, PLAN §11).
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  createDeviceSet,
  devicesQuery,
  doctorQuery,
  presetsQuery,
  STATE_KEY,
  stateQuery,
} from "../lib/api";
import type { DeviceInfo, DeviceSet } from "../lib/types";
import { BTN } from "./controls";
import { TemplatesPanel } from "./TemplatesPanel";

const DISMISSED_KEY = "sdrmm.wizard.v1.dismissed";

function readDismissed(): boolean {
  try {
    return window.localStorage.getItem(DISMISSED_KEY) === "1";
  } catch {
    // A blocked store just means the wizard reappears next time; that is the safe direction.
    return false;
  }
}

function writeDismissed(): void {
  try {
    window.localStorage.setItem(DISMISSED_KEY, "1");
  } catch {
    // Ignored for the same reason.
  }
}

function deviceRank(device: DeviceInfo): number {
  return device.driver === "virtual" ? 1 : 0;
}

/** Hardware first, then the virtual devices — a first-time user with a dongle attached should
 * not have to scroll past the signal generator to find it. */
export function rankDevices(devices: readonly DeviceInfo[]): readonly DeviceInfo[] {
  return devices.toSorted(
    (a, b) => deviceRank(a) - deviceRank(b) || a.label.localeCompare(b.label),
  );
}

export function FirstRun({
  active,
  onSelectDeviceSet,
}: {
  active: DeviceSet | null;
  onSelectDeviceSet: (ds: number) => void;
}) {
  const queryClient = useQueryClient();
  const [dismissed, setDismissed] = useState(readDismissed);
  // The wizard's own first step makes the server non-empty, so "untouched" stops being true
  // the moment it succeeds; this keeps the template step on screen until the user is done.
  const [started, setStarted] = useState(false);
  const state = useQuery(stateQuery());
  const presets = useQuery(presetsQuery());
  const devices = useQuery(devicesQuery());
  const [error, setError] = useState<string | null>(null);
  const [showDoctor, setShowDoctor] = useState(false);

  const openMut = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: (id) => {
      onSelectDeviceSet(id);
      setStarted(true);
      setError(null);
    },
    onError: (e) => setError(e.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  // Never guess while the answer is still loading: a wizard that flashes over a configured
  // station is worse than one that appears a beat late.
  const untouched =
    state.data !== undefined &&
    presets.data !== undefined &&
    state.data.device_sets.length === 0 &&
    presets.data.length === 0;
  if (dismissed || (!untouched && !started)) {
    return null;
  }

  const dismiss = () => {
    writeDismissed();
    setDismissed(true);
  };

  return (
    <div className="border-b border-line bg-panel px-4 py-3">
      <div className="flex items-baseline justify-between gap-3">
        <h2 className="text-sm font-semibold uppercase tracking-wider text-accent">
          Welcome to sdr--
        </h2>
        <button type="button" className="text-xs text-ink-dim underline" onClick={dismiss}>
          skip
        </button>
      </div>

      {active === null ? (
        <>
          <p className="mt-1 text-sm text-ink-dim">
            Pick a receiver to open. No hardware? The signal generator and any recording play back
            exactly like a device.
          </p>
          <div className="mt-2 flex flex-wrap gap-2">
            {rankDevices(devices.data?.devices ?? []).map((d) => (
              <button
                key={`${d.driver}:${d.key}`}
                type="button"
                className={BTN}
                disabled={openMut.isPending}
                onClick={() => openMut.mutate(`${d.driver}:${d.key}`)}
              >
                {d.label}
              </button>
            ))}
          </div>
          {(devices.data?.devices.length ?? 0) === 0 && (
            <p className="mt-2 text-sm text-ink-dim">No devices found.</p>
          )}
          <button
            type="button"
            className="mt-2 text-xs text-ink-dim underline"
            onClick={() => setShowDoctor(!showDoctor)}
          >
            {showDoctor ? "hide diagnostics" : "hardware not showing up?"}
          </button>
          {showDoctor && <Doctor />}
        </>
      ) : (
        <>
          <p className="mt-1 text-sm text-ink-dim">
            {active.device.label} is open. Pick something to listen to — you can change everything
            afterwards.
          </p>
          <TemplatesPanel active={active} onApplied={dismiss} />
        </>
      )}

      {error !== null && (
        <div role="alert" className="mt-2 font-mono text-sm text-danger">
          {error}
        </div>
      )}
    </div>
  );
}

/** The `--doctor` report, rendered where a stuck first-time user will actually look for it. */
function Doctor() {
  const doctor = useQuery(doctorQuery(true));
  if (doctor.isPending) {
    return <p className="mt-2 text-sm text-ink-dim">Checking…</p>;
  }
  if (doctor.error) {
    return (
      <p className="mt-2 font-mono text-sm text-danger">
        Diagnostics failed: {doctor.error.message}
      </p>
    );
  }
  return (
    <dl className="mt-2 flex flex-col gap-1">
      {(doctor.data?.checks ?? []).map((check) => (
        <div key={check.id} className="text-xs">
          <dt
            className={`font-mono ${
              check.status === "fail"
                ? "text-danger"
                : check.status === "warn"
                  ? "text-ink"
                  : "text-ink-dim"
            }`}
          >
            [{check.status}] {check.name}
          </dt>
          <dd className="whitespace-pre-wrap pl-4 font-mono text-[10px] text-ink-dim">
            {check.detail}
            {check.hint != null && `\n→ ${check.hint}`}
          </dd>
        </div>
      ))}
    </dl>
  );
}
