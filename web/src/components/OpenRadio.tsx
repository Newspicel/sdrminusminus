import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { devicesQuery, doctorQuery } from "../lib/api";
import type { DeviceInfo } from "../lib/types";
import { LABEL } from "./controls";
import { EmptyState } from "./EmptyState";
import { InlineAlert } from "./InlineAlert";
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
    <form
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
          className="w-full"
          value={driver}
          options={NETWORK_BACKENDS.map((b) => ({ value: b.driver, label: b.label }))}
          onChange={setDriver}
        />
      </div>
      <div className="flex items-center gap-2">
        <Input
          className="w-full"
          type="text"
          aria-label="Radio address"
          placeholder={backend.placeholder}
          value={address}
          onChange={(event) => setAddress(event.target.value)}
        />
        <Button type="submit" variant="outline" size="sm" disabled={busy || id === null}>
          Add
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        The port may be left off — {backend.label} defaults to{" "}
        {backend.placeholder.split(":").pop()}.
      </p>
    </form>
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
    <Dialog
      onOpenChange={(open) => {
        if (!open) setQuery("");
      }}
    >
      <DialogTrigger
        render={<Button variant="outline" size="sm" className="justify-center" disabled={busy} />}
      >
        Recordings ({recordings.length})
      </DialogTrigger>
      <DialogContent className="flex max-h-[80vh] max-w-lg flex-col" showCloseButton={false}>
        <DialogTitle className="text-base font-medium text-foreground">Recordings</DialogTitle>
        <DialogDescription className="mt-1 text-xs text-muted-foreground">
          Choose a saved IQ recording to open as a source.
        </DialogDescription>
        <Input
          className="mt-3 w-full shrink-0"
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
              variant="outline"
              size="sm"
              className="h-auto min-h-7 shrink-0 justify-start py-1.5 text-left"
              disabled={busy}
              onClick={() => onChoose(device)}
            >
              <span className="truncate">{device.label}</span>
            </Button>
          ))}
          {recordings.length === 0 && <EmptyState>No recordings yet.</EmptyState>}
          {recordings.length > 0 && filtered.length === 0 && (
            <EmptyState>No matching recordings.</EmptyState>
          )}
        </div>
        <DialogFooter className="mt-4 shrink-0">
          <DialogClose render={<Button variant="outline" size="sm" />}>Close</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
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
}: {
  onChoose: (device: DeviceInfo) => void;
  onAddNetwork: (deviceId: string) => void;
  busy?: boolean;
  error?: string | null;
}) {
  const devices = useQuery(devicesQuery());
  const [showDoctor, setShowDoctor] = useState(false);
  const [showNetwork, setShowNetwork] = useState(false);
  const found = visibleDevices(devices.data?.devices ?? []);
  const { radios, recordings } = groupDevices(found);

  return (
    <div className="flex w-full flex-col gap-2">
      <div className="flex flex-col gap-1">
        {radios.map((device, index) => (
          <Button
            key={deviceId(device)}
            type="button"
            variant={index === 0 ? "default" : "outline"}
            size="sm"
            className="justify-center"
            disabled={busy}
            onClick={() => onChoose(device)}
          >
            <span className="truncate">{device.label}</span>
          </Button>
        ))}
      </div>

      {devices.isPending && <Skeleton className="h-8 w-full" />}
      {!devices.isPending && radios.length === 0 && (
        <p className="text-sm text-muted-foreground">
          No radios found. Plug one in, open a recording, or check the diagnostics below.
        </p>
      )}

      {error !== null && <InlineAlert className="font-mono text-xs">{error}</InlineAlert>}

      <RecordingChoices recordings={recordings} onChoose={onChoose} busy={busy} />

      <Button
        type="button"
        variant="ghost"
        size="sm"
        className="self-center"
        onClick={() => setShowNetwork(!showNetwork)}
      >
        {showNetwork ? "Hide network radio" : "Radio on the network?"}
      </Button>
      {showNetwork && <AddNetworkRadio onAdd={onAddNetwork} busy={busy} />}

      <Button
        type="button"
        variant="ghost"
        size="sm"
        className="self-center"
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
    return <p className="text-sm text-muted-foreground">Checking…</p>;
  }
  if (doctor.error) {
    return (
      <InlineAlert className="font-mono text-xs">
        Diagnostics failed: {doctor.error.message}
      </InlineAlert>
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
                  ? "text-destructive"
                  : check.status === "warn"
                    ? "text-foreground"
                    : "text-success"
              }
            >
              [{check.status}]
            </span>
            <span className="text-foreground">{check.name}</span>
          </dt>
          <dd className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70 pl-4 whitespace-pre-wrap normal-case">
            {check.detail}
            {check.hint != null && `\n→ ${check.hint}`}
          </dd>
        </div>
      ))}
    </dl>
  );
}
