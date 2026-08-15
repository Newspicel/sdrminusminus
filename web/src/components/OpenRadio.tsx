import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { refMatches } from "../canvas/binding";
import { devicesQuery, doctorQuery } from "../lib/api";
import type { DeviceInfo, DeviceRef } from "../lib/types";
import { Button, Form, Input } from "./BaseControls";
import { BTN, BTN_PRIMARY, BTN_QUIET, FIELD, LABEL, SURFACE } from "./controls";
import { Select } from "./Select";

function deviceRank(device: DeviceInfo): number {
  return device.driver === "virtual" ? 1 : 0;
}

export function isRecordingDevice(device: DeviceInfo): boolean {
  return device.driver === "virtual" && device.key.startsWith("file:");
}

/** Hardware first, then the virtual devices — someone with a dongle attached should not have to
 * read past the signal generator to find it. */
export function rankDevices(devices: readonly DeviceInfo[]): readonly DeviceInfo[] {
  return devices.toSorted(
    (a, b) => deviceRank(a) - deviceRank(b) || a.label.localeCompare(b.label),
  );
}

export function visibleDevices(
  devices: readonly DeviceInfo[],
  showSynthetic = import.meta.env.DEV || import.meta.env.VITE_ENABLE_SYNTHETIC_DEVICES === "true",
): readonly DeviceInfo[] {
  return rankDevices(
    showSynthetic
      ? devices
      : devices.filter((device) => device.driver !== "virtual" || isRecordingDevice(device)),
  );
}

/** The devices still free to be named here: one radio is one open device set behind one node, so
 * the ones another node already holds are not a choice this node can make (`claimedDevices`). */
export function unclaimedDevices(
  devices: readonly DeviceInfo[],
  claimed: readonly DeviceRef[],
): readonly DeviceInfo[] {
  return devices.filter((device) => !claimed.some((reference) => refMatches(reference, device)));
}

export function groupDevices(devices: readonly DeviceInfo[]): {
  radios: readonly DeviceInfo[];
  recordings: readonly DeviceInfo[];
} {
  return {
    radios: devices.filter((device) => !isRecordingDevice(device)),
    recordings: devices.filter(isRecordingDevice),
  };
}

export function filterRecordingDevices(
  recordings: readonly DeviceInfo[],
  query: string,
): readonly DeviceInfo[] {
  const normalized = query.trim().toLowerCase();
  return normalized === ""
    ? recordings
    : recordings.filter((recording) => recording.label.toLowerCase().includes(normalized));
}

export function deviceId(device: DeviceInfo): string {
  return `${device.driver}:${device.key}`;
}

/** The protocols a radio elsewhere on the network can be reached over. Both are named, never
 * discovered — neither has any discovery — so this list is also the whole of what the picker can
 * offer before an address is typed. */
export const NETWORK_BACKENDS = [
  { driver: "rtltcp", label: "rtl_tcp", placeholder: "192.168.1.5:1234" },
  { driver: "spyserver", label: "SpyServer", placeholder: "192.168.1.5:5555" },
] as const;

/** The `driver:key` that opens a network radio, or `null` when there is nothing usable to send.
 *
 * Only the refusals that need no knowledge are made here — an empty address, one with a space in
 * it. What the key *canonicalizes* to is the server's to decide: it defaults the port and
 * lower-cases the host, and the caller learns the result back from the device the open returns.
 * Deciding it here would be a second address parser to keep in step with the backend's, and the
 * patch would then store a key the probe never reports. */
export function networkDeviceId(driver: string, address: string): string | null {
  // A pasted `rtl_tcp://host:1234` is the address with a scheme in front of it; an IPv6 literal
  // never matches, because a scheme needs the slashes. The underscore is not one a URL scheme may
  // contain, but it is what people type for this one.
  const trimmed = address.trim().replace(/^[a-z][a-z0-9+._-]*:\/\//i, "");
  if (trimmed === "" || /\s/.test(trimmed)) {
    return null;
  }
  return `${driver}:${trimmed}`;
}

/** Naming a radio that is somewhere else. Folded away by default: it is the rarer path, and an
 * address field above the list of what is actually plugged in would read as the main one. */
function AddNetworkRadio({ onAdd, busy }: { onAdd: (id: string) => void; busy: boolean }) {
  const [driver, setDriver] = useState<string>(NETWORK_BACKENDS[0].driver);
  const [address, setAddress] = useState("");
  const backend = NETWORK_BACKENDS.find((b) => b.driver === driver) ?? NETWORK_BACKENDS[0];
  const id = networkDeviceId(driver, address);

  return (
    <Form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (id !== null) {
          onAdd(id);
        }
      }}
    >
      <div className="flex items-center gap-2">
        <span className={LABEL}>Via</span>
        <Select
          label="Network protocol"
          value={driver}
          options={NETWORK_BACKENDS.map((b) => ({ value: b.driver, label: b.label }))}
          onChange={setDriver}
        />
      </div>
      <div className="flex items-center gap-2">
        <Input
          className={`${FIELD} w-full`}
          type="text"
          aria-label="Radio address"
          placeholder={backend.placeholder}
          value={address}
          onChange={(event) => setAddress(event.target.value)}
        />
        <Button type="submit" className={BTN} disabled={busy || id === null}>
          Add
        </Button>
      </div>
      <p className="text-xs text-ink-dim">
        The port may be left off — {backend.label} defaults to{" "}
        {backend.placeholder.split(":").pop()}.
      </p>
    </Form>
  );
}

function RecordingChoices({
  recordings,
  onChoose,
  busy,
}: {
  recordings: readonly DeviceInfo[];
  onChoose: (device: DeviceInfo) => void;
  busy: boolean;
}) {
  const [query, setQuery] = useState("");
  const filtered = filterRecordingDevices(recordings, query);

  return (
    <Dialog.Root
      onOpenChange={(open) => {
        if (!open) setQuery("");
      }}
    >
      <Dialog.Trigger className={`${BTN} justify-center`} disabled={busy}>
        Recordings ({recordings.length})
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex max-h-[80vh] w-full max-w-lg -translate-x-1/2 -translate-y-1/2 flex-col p-4`}
        >
          <Dialog.Title className="text-base font-medium text-ink">Recordings</Dialog.Title>
          <Dialog.Description className="mt-1 text-xs text-ink-dim">
            Choose a saved IQ recording to open as a source.
          </Dialog.Description>
          <Input
            className={`${FIELD} mt-3 w-full shrink-0`}
            type="search"
            name="recording-filter"
            placeholder="Search recordings"
            aria-label="Search recordings"
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="mt-2 flex min-h-0 flex-col gap-1 overflow-y-auto">
            {filtered.map((device) => (
              <Button
                key={deviceId(device)}
                type="button"
                className={`${BTN} h-auto min-h-7 shrink-0 justify-start py-1.5 text-left`}
                disabled={busy}
                onClick={() => onChoose(device)}
              >
                <span className="truncate">{device.label}</span>
              </Button>
            ))}
            {recordings.length === 0 && (
              <p className="py-3 text-center text-sm text-ink-dim">No recordings yet.</p>
            )}
            {recordings.length > 0 && filtered.length === 0 && (
              <p className="py-3 text-center text-sm text-ink-dim">No matching recordings.</p>
            )}
          </div>
          <div className="mt-4 flex shrink-0 justify-end">
            <Dialog.Close className={BTN}>Close</Dialog.Close>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** The discovered devices, one button each, with the states discovery itself can be in — plus the
 * one radio no discovery can find, which is the one on another machine. The caller decides what
 * choosing one does. */
export function DeviceChoices({
  onChoose,
  onAddNetwork,
  busy = false,
  error = null,
  claimed = [],
}: {
  onChoose: (device: DeviceInfo) => void;
  onAddNetwork: (deviceId: string) => void;
  busy?: boolean;
  error?: string | null;
  /** Radios other nodes already name, which this one may not name a second time. */
  claimed?: readonly DeviceRef[];
}) {
  const devices = useQuery(devicesQuery());
  const [showDoctor, setShowDoctor] = useState(false);
  const [showNetwork, setShowNetwork] = useState(false);
  const visible = visibleDevices(devices.data?.devices ?? []);
  const found = unclaimedDevices(visible, claimed);
  const { radios, recordings } = groupDevices(found);
  const elsewhere = visible.length - found.length;

  return (
    <div className="flex w-full flex-col gap-2">
      <div className="flex flex-col gap-1">
        {radios.map((device, index) => (
          <Button
            key={deviceId(device)}
            type="button"
            className={`${index === 0 ? BTN_PRIMARY : BTN} justify-center`}
            disabled={busy}
            onClick={() => onChoose(device)}
          >
            <span className="truncate">{device.label}</span>
          </Button>
        ))}
      </div>

      {devices.isPending && <p className="text-sm text-ink-dim">Looking for devices…</p>}
      {!devices.isPending && radios.length === 0 && (
        <p className="text-sm text-ink-dim">
          {elsewhere > 0
            ? "Every radio found is already open on another node. Plug one in, open a recording, or move that node's wires here."
            : "No radios found. Plug one in, open a recording, or check the diagnostics below."}
        </p>
      )}

      {error !== null && (
        <p role="alert" className="font-mono text-xs text-danger">
          {error}
        </p>
      )}

      <RecordingChoices recordings={recordings} onChoose={onChoose} busy={busy} />

      <Button
        type="button"
        className={`${BTN_QUIET} self-center`}
        onClick={() => setShowNetwork(!showNetwork)}
      >
        {showNetwork ? "Hide network radio" : "Radio on the network?"}
      </Button>
      {showNetwork && <AddNetworkRadio onAdd={onAddNetwork} busy={busy} />}

      <Button
        type="button"
        className={`${BTN_QUIET} self-center`}
        onClick={() => setShowDoctor(!showDoctor)}
      >
        {showDoctor ? "Hide diagnostics" : "Hardware not showing up?"}
      </Button>
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
