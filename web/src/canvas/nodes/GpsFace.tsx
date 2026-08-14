import { useQuery } from "@tanstack/react-query";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { LABEL } from "../../components/controls";
import { Select } from "../../components/Select";
import { TextAutocomplete } from "../../components/TextAutocomplete";
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
        <div className="flex flex-col gap-2 p-2">
          <SourceSettings source={source} onChange={setSource} />
          {fix === null ? (
            <span className="text-xs text-muted-foreground">
              {state?.error ?? "Waiting for a fix…"}
            </span>
          ) : (
            <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
              <span className={LABEL}>Position</span>
              <span className="text-right font-mono tabular-nums">
                {fix.latitude.toFixed(6)}, {fix.longitude.toFixed(6)}
              </span>
              <span className={LABEL}>Grid</span>
              <span className="text-right font-mono tabular-nums">
                {gridLocator(fix.latitude, fix.longitude)}
              </span>
              {fix.accuracy_m != null && (
                <>
                  <span className={LABEL}>Accuracy</span>
                  <span className="text-right font-mono tabular-nums">
                    ±{fix.accuracy_m.toFixed(0)} m
                  </span>
                </>
              )}
              {fix.speed_mps != null && (
                <>
                  <span className={LABEL}>Speed</span>
                  <span className="text-right font-mono tabular-nums">
                    {(fix.speed_mps * 3.6).toFixed(1)} km/h
                  </span>
                </>
              )}
            </div>
          )}
        </div>
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
        <span className="text-xs text-muted-foreground">This device's live location provider</span>
      );
    case "gpsd":
      return (
        <Label className={LABEL}>
          GPSD address
          <Input
            key={source.address}
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
        </Label>
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
    <div className="flex flex-col gap-2">
      <div className={LABEL}>
        <span>Serial device</span>
        <TextAutocomplete
          value={source.device}
          label="Serial device"
          className="flex-1"
          placeholder="Choose a detected device or enter a path"
          suggestions={(devices.data?.devices ?? []).map((device) => ({
            value: device.path,
            detail: nmeaDeviceLabel(device),
          }))}
          onCommit={(device) => {
            if (device !== source.device) {
              onChange({ ...source, device, update_interval_ms: updateInterval });
            }
            return true;
          }}
        />
      </div>
      {devices.isError && (
        <span className="text-[10px] text-destructive">Serial device discovery failed</span>
      )}
      <div className="grid grid-cols-2 gap-2">
        <div className={LABEL}>
          <span>Baud</span>
          <TextAutocomplete
            value={String(source.baud)}
            label="Baud"
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
        </div>
        <Label className={LABEL}>
          Update rate
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
        </Label>
      </div>
      <span className="text-[10px] text-muted-foreground/70">
        NMEA receivers push sentences; update rate limits how often fixes are published.
      </span>
    </div>
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

function nmeaDeviceLabel(device: NmeaDeviceInfo): string {
  const description = device.product ?? device.manufacturer;
  const serial = device.serial == null ? "" : ` · ${device.serial}`;
  return description == null ? device.path : `${description}${serial}`;
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
