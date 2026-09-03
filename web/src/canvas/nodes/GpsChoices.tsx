import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { Button, Form, Input } from "../../components/BaseControls";
import { BTN, BTN_QUIET, FIELD, LABEL } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { nmeaDevicesQuery } from "../../lib/api";
import type { PositionSource } from "../../lib/types";
import { filterNmeaDevices, nmeaDetail, nmeaSource, validGpsdAddress } from "./gpsSource";

const SEARCH_FROM = 4;

export function GpsChoices({ onChoose }: { onChoose: (source: PositionSource) => void }) {
  const devices = useQuery(nmeaDevicesQuery());
  const [query, setQuery] = useState("");
  const [showFixed, setShowFixed] = useState(false);
  const [showNetwork, setShowNetwork] = useState(false);
  const [showPath, setShowPath] = useState(false);
  const listed = devices.data?.devices ?? [];
  const found = filterNmeaDevices(listed, query);

  return (
    <div className="flex w-full flex-col gap-2">
      {listed.length >= SEARCH_FROM && (
        <Input
          className={`${FIELD} w-full`}
          type="search"
          name="gps-filter"
          placeholder="Search receivers"
          aria-label="Search receivers"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      )}

      <div className="flex flex-col gap-1">
        {found.map((device) => {
          const detail = nmeaDetail(device);
          return (
            <Button
              key={device.path}
              type="button"
              className={`${BTN} h-auto min-h-7 justify-start py-1.5 text-left`}
              onClick={() => onChoose(nmeaSource(device.path))}
            >
              <span className="flex w-full min-w-0 flex-col gap-0.5">
                <span className="truncate">{device.path}</span>
                {detail !== "" && (
                  <span className="truncate font-mono text-[10px] text-ink-faint">{detail}</span>
                )}
              </span>
            </Button>
          );
        })}
      </div>

      {devices.isPending && <p className="text-ink-dim text-sm">Looking for receivers…</p>}
      {!devices.isPending && found.length === 0 && (
        <p className="text-ink-dim text-sm">
          {listed.length > 0
            ? "No receiver matches that."
            : "No serial receiver found. Plug one in, take this device's own location, or type a place."}
        </p>
      )}
      {devices.isError && (
        <p role="alert" className="font-mono text-danger text-xs">
          Serial device discovery failed
        </p>
      )}

      {navigator.geolocation !== undefined && (
        <Button
          type="button"
          className={`${BTN} justify-center`}
          onClick={() => onChoose({ type: "device" })}
        >
          This device's location
        </Button>
      )}

      <Disclosure
        open={showPath}
        onToggle={() => setShowPath(!showPath)}
        closed="Receiver not listed?"
        opened="Hide serial path"
      >
        <SerialPathForm onChoose={onChoose} />
      </Disclosure>

      <Disclosure
        open={showNetwork}
        onToggle={() => setShowNetwork(!showNetwork)}
        closed="GPS on the network?"
        opened="Hide network GPS"
      >
        <GpsdForm onChoose={onChoose} />
      </Disclosure>

      <Disclosure
        open={showFixed}
        onToggle={() => setShowFixed(!showFixed)}
        closed="Receiver that never moves?"
        opened="Hide fixed place"
      >
        <FixedForm onChoose={onChoose} />
      </Disclosure>
    </div>
  );
}

function Disclosure({
  open,
  onToggle,
  closed,
  opened,
  children,
}: {
  open: boolean;
  onToggle: () => void;
  closed: string;
  opened: string;
  children: ReactNode;
}) {
  return (
    <>
      <Button
        type="button"
        className={`${BTN_QUIET} self-center`}
        aria-expanded={open}
        onClick={onToggle}
      >
        {open ? opened : closed}
      </Button>
      {open && children}
    </>
  );
}

function SerialPathForm({ onChoose }: { onChoose: (source: PositionSource) => void }) {
  const [path, setPath] = useState("");
  const trimmed = path.trim();
  return (
    <Form
      className="flex items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (trimmed !== "") {
          onChoose(nmeaSource(trimmed));
        }
      }}
    >
      <Input
        className={`${FIELD} w-full`}
        type="text"
        aria-label="Serial device path"
        placeholder="/dev/ttyUSB0"
        value={path}
        onChange={(event) => setPath(event.target.value)}
      />
      <Button type="submit" className={BTN} disabled={trimmed === ""}>
        Read
      </Button>
    </Form>
  );
}

function GpsdForm({ onChoose }: { onChoose: (source: PositionSource) => void }) {
  const [address, setAddress] = useState("127.0.0.1:2947");
  const valid = validGpsdAddress(address.trim());
  return (
    <Form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (valid) {
          onChoose({ type: "gpsd", address: address.trim() });
        }
      }}
    >
      <div className="flex items-center gap-2">
        <Input
          className={`${FIELD} w-full`}
          type="text"
          aria-label="GPSD address"
          placeholder="127.0.0.1:2947"
          value={address}
          onChange={(event) => setAddress(event.target.value)}
        />
        <Button type="submit" className={BTN} disabled={!valid}>
          Read
        </Button>
      </div>
      <p className="text-ink-dim text-xs">
        A gpsd daemon on this machine or another one, host and port.
      </p>
    </Form>
  );
}

function FixedForm({ onChoose }: { onChoose: (source: PositionSource) => void }) {
  const [lat, setLat] = useState(0);
  const [lon, setLon] = useState(0);
  return (
    <Form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        onChoose({ type: "fixed", lat, lon });
      }}
    >
      <div className="flex items-center gap-2">
        <span className={LABEL}>Lat</span>
        <NumberField
          label="Latitude in degrees"
          value={lat}
          min={-90}
          max={90}
          step={0.00001}
          onCommit={setLat}
          className="w-24 text-center"
        />
        <span className={LABEL}>Lon</span>
        <NumberField
          label="Longitude in degrees"
          value={lon}
          min={-180}
          max={180}
          step={0.00001}
          onCommit={setLon}
          className="w-24 text-center"
        />
      </div>
      <Button type="submit" className={`${BTN} justify-center`}>
        Stand here
      </Button>
    </Form>
  );
}
