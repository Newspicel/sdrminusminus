import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { devicesQuery, doctorQuery } from "../lib/api";
import type { DeviceInfo, DeviceRef } from "../lib/types";
import { Button, Form, Input } from "./BaseControls";
import { BTN, BTN_PRIMARY, BTN_QUIET, FIELD, LABEL, SURFACE } from "./controls";
import {
  deviceId,
  filterRecordingDevices,
  groupDevices,
  NETWORK_BACKENDS,
  networkDeviceId,
  unclaimedDevices,
  visibleDevices,
} from "./devices";
import { Select } from "./Select";

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
