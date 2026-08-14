import { useQuery } from "@tanstack/react-query";
import { FIELD, LABEL } from "../../components/controls";
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
          <SourceSettings source={source} onChange={setSource} listId={`nmea-${node.id}`} />
          {fix === null ? (
            <span className="text-xs text-ink-dim">{state?.error ?? "Waiting for a fix…"}</span>
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
  listId,
}: {
  source: PositionSource;
  onChange: (source: PositionSource) => void;
  listId: string;
}) {
  switch (source.type) {
    case "device":
      return <span className="text-xs text-ink-dim">This device's live location provider</span>;
    case "gpsd":
      return (
        <label className={LABEL}>
          GPSD address
          <input
            key={source.address}
            className={FIELD}
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
        </label>
      );
    case "nmea":
      return <NmeaSettings source={source} onChange={onChange} listId={listId} />;
  }
}

function NmeaSettings({
  source,
  onChange,
  listId,
}: {
  source: Extract<PositionSource, { type: "nmea" }>;
  onChange: (source: PositionSource) => void;
  listId: string;
}) {
  const devices = useQuery(nmeaDevicesQuery());
  const updateInterval = source.update_interval_ms ?? 1_000;
  return (
    <div className="flex flex-col gap-2">
      <label className={LABEL}>
        Serial device
        <input
          key={source.device}
          className={FIELD}
          list={listId}
          defaultValue={source.device}
          placeholder="Choose a detected device or enter a path"
          onBlur={(event) => {
            const device = event.currentTarget.value.trim();
            if (device === "") {
              event.currentTarget.value = source.device;
            } else if (device !== source.device) {
              onChange({ ...source, device, update_interval_ms: updateInterval });
            } else {
              event.currentTarget.value = source.device;
            }
          }}
        />
        <datalist id={listId}>
          {devices.data?.devices.map((device) => (
            <option key={device.path} value={device.path} label={nmeaDeviceLabel(device)} />
          ))}
        </datalist>
      </label>
      {devices.isError && (
        <span className="text-[10px] text-danger">Serial device discovery failed</span>
      )}
      <div className="grid grid-cols-2 gap-2">
        <label className={LABEL}>
          Baud
          <input
            key={source.baud}
            className={FIELD}
            type="number"
            min={1_200}
            max={4_000_000}
            list={`${listId}-bauds`}
            defaultValue={source.baud}
            onBlur={(event) => {
              const baud = Number(event.currentTarget.value);
              if (
                Number.isInteger(baud) &&
                baud >= 1_200 &&
                baud <= 4_000_000 &&
                baud !== source.baud
              ) {
                onChange({ ...source, baud, update_interval_ms: updateInterval });
              } else {
                event.currentTarget.value = String(source.baud);
              }
            }}
          />
          <datalist id={`${listId}-bauds`}>
            {[4_800, 9_600, 38_400, 57_600, 115_200].map((baud) => (
              <option key={baud} value={baud} />
            ))}
          </datalist>
        </label>
        <label className={LABEL}>
          Update rate
          <select
            className={FIELD}
            value={updateInterval}
            onChange={(event) =>
              onChange({ ...source, update_interval_ms: Number(event.currentTarget.value) })
            }
          >
            <option value={1_000}>1 Hz</option>
            <option value={500}>2 Hz</option>
            <option value={200}>5 Hz</option>
            <option value={100}>10 Hz</option>
            <option value={50}>20 Hz</option>
          </select>
        </label>
      </div>
      <span className="text-[10px] text-ink-faint">
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
    return /^\[[0-9a-f:]+\]$/i.test(host) && host.includes(":");
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
