import { useQuery } from "@tanstack/react-query";
import { Input } from "../../components/BaseControls";
import { FIELD } from "../../components/controls";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Select } from "../../components/Select";
import { SettingNote, SettingRow, Settings } from "../../components/Settings";
import { type AutocompleteSuggestion, TextAutocomplete } from "../../components/TextAutocomplete";
import { nmeaDevicesQuery } from "../../lib/api";
import { gridLocator, usePositionStore } from "../../lib/position";
import type { NmeaDeviceInfo, PatchNode, PositionSource } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";

export function GpsFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = usePositionStore((store) => store.sources[node.id]);
  if (node.kind !== "gps") {
    return null;
  }
  const source: PositionSource = node.data.source ?? { type: "device" };
  const setSource = (next: PositionSource): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "gps" ? { ...current, data: { source: next } } : current,
      ),
    }));
  };
  const fix = state?.fix ?? null;
  return (
    <NodeShell
      node={node}
      title="GPS position"
      category="source"
      subtitle={sourceName(source)}
      live={fix !== null}
    >
      <FaceBody>
        <Settings className="p-2">
          <SourceSettings source={source} onChange={setSource} />
        </Settings>
        {fix === null ? (
          <p className="border-t border-line p-2 text-xs text-ink-dim">
            {state?.error ?? "Waiting for a fix…"}
          </p>
        ) : (
          <Readout>
            <ReadoutRow label="Position">
              {fix.latitude.toFixed(6)}, {fix.longitude.toFixed(6)}
            </ReadoutRow>
            <ReadoutRow label="Grid">{gridLocator(fix.latitude, fix.longitude)}</ReadoutRow>
            {fix.accuracy_m != null && (
              <ReadoutRow label="Accuracy">±{fix.accuracy_m.toFixed(0)} m</ReadoutRow>
            )}
            {fix.speed_mps != null && (
              <ReadoutRow label="Speed">{(fix.speed_mps * 3.6).toFixed(1)} km/h</ReadoutRow>
            )}
          </Readout>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function SourceSettings({
  source,
  onChange,
}: {
  source: PositionSource;
  onChange: (source: PositionSource) => void;
}) {
  switch (source.type) {
    case "device":
      return (
        <SettingRow label="Source">
          <span className="text-xs text-ink-dim">This device's live location provider</span>
        </SettingRow>
      );
    case "gpsd":
      return (
        <SettingRow label="GPSD address">
          <Input
            key={source.address}
            aria-label="GPSD address"
            className={`${FIELD} w-full max-w-52`}
            defaultValue={source.address}
            onBlur={(event) => {
              const address = event.currentTarget.value.trim();
              if (!validGpsdAddress(address)) {
                event.currentTarget.value = source.address;
              } else if (address !== source.address) {
                onChange({ type: "gpsd", address });
              } else {
                event.currentTarget.value = source.address;
              }
            }}
          />
        </SettingRow>
      );
    case "nmea":
      return <NmeaSettings source={source} onChange={onChange} />;
  }
}

function NmeaSettings({
  source,
  onChange,
}: {
  source: Extract<PositionSource, { type: "nmea" }>;
  onChange: (source: PositionSource) => void;
}) {
  const devices = useQuery(nmeaDevicesQuery());
  const updateInterval = source.update_interval_ms ?? 1_000;
  return (
    <>
      <SettingRow label="Serial device">
        <TextAutocomplete
          value={source.device}
          label="Serial device"
          className="w-full max-w-52"
          placeholder="Choose a detected device or enter a path"
          suggestions={(devices.data?.devices ?? []).map(nmeaSuggestion)}
          onCommit={(device) => {
            if (device !== source.device) {
              onChange({ ...source, device, update_interval_ms: updateInterval });
            }
            return true;
          }}
        />
      </SettingRow>
      {devices.isError && (
        <p className="col-span-2 text-xs text-danger">Serial device discovery failed</p>
      )}
      {devices.isSuccess && devices.data.devices.length === 0 && (
        <SettingNote>No serial receiver detected — plug one in, or type its path.</SettingNote>
      )}
      <SettingRow label="Baud">
        <TextAutocomplete
          value={String(source.baud)}
          label="Baud"
          className="w-full max-w-52"
          inputMode="numeric"
          suggestions={[4_800, 9_600, 38_400, 57_600, 115_200].map((baud) => ({
            value: String(baud),
          }))}
          onCommit={(value) => {
            const baud = Number(value);
            const valid = Number.isInteger(baud) && baud >= 1_200 && baud <= 4_000_000;
            if (valid && baud !== source.baud) {
              onChange({ ...source, baud, update_interval_ms: updateInterval });
            }
            return valid;
          }}
        />
      </SettingRow>
      <SettingRow label="Update rate">
        <Select
          label="Update rate"
          value={updateInterval}
          options={[
            { value: 1_000, label: "1 Hz" },
            { value: 500, label: "2 Hz" },
            { value: 200, label: "5 Hz" },
            { value: 100, label: "10 Hz" },
            { value: 50, label: "20 Hz" },
          ]}
          onChange={(next) => onChange({ ...source, update_interval_ms: next })}
        />
      </SettingRow>
    </>
  );
}

export function validGpsdAddress(address: string): boolean {
  const separator = address.lastIndexOf(":");
  if (separator <= 0) {
    return false;
  }
  const host = address.slice(0, separator);
  const port = Number(address.slice(separator + 1));
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    return false;
  }
  if (host.startsWith("[") || host.endsWith("]")) {
    try {
      const parsed = new URL(`http://${host}`);
      return parsed.hostname.startsWith("[") && parsed.hostname.endsWith("]");
    } catch {
      return false;
    }
  }
  return /^[a-z0-9._-]+$/i.test(host);
}

/** A detected port as one suggestion: the path is the value, and whatever the USB descriptor says
 * is behind it is the second line — left off, rather than repeating the path, for a port that
 * reports no identity of its own. */
export function nmeaSuggestion(device: NmeaDeviceInfo): AutocompleteSuggestion {
  const description = device.product ?? device.manufacturer;
  if (description == null) {
    return { value: device.path };
  }
  const serial = device.serial == null ? "" : ` · ${device.serial}`;
  return { value: device.path, detail: `${description}${serial}` };
}

function sourceName(source: PositionSource): string {
  switch (source.type) {
    case "device":
      return "device";
    case "gpsd":
      return "gpsd";
    case "nmea":
      return "NMEA";
  }
}
