import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button, Form, Input } from "../../components/BaseControls";
import { BTN, FIELD, LABEL } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Segmented } from "../../components/Segmented";
import { nmeaDevicesQuery, phonesQuery } from "../../lib/api";
import { STANDING_LABEL } from "../../lib/phones";
import type { PositionSource } from "../../lib/types";
import {
  filterNmeaDevices,
  type GpsTab,
  gpsTabs,
  nmeaDetail,
  nmeaSource,
  validGpsdAddress,
} from "./gpsSource";

const SEARCH_FROM = 4;

type Choose = (source: PositionSource) => void;

export function GpsChoices({ onChoose }: { onChoose: Choose }) {
  const [tab, setTab] = useState<GpsTab>("receiver");
  const tabs = gpsTabs();

  return (
    <div className="flex w-full flex-col gap-2">
      <Segmented label="Position source" value={tab} options={tabs} onChange={setTab} fill />
      {tab === "receiver" && <ReceiverChoices onChoose={onChoose} />}
      {tab === "network" && <GpsdForm onChoose={onChoose} />}
      {tab === "fixed" && <FixedForm onChoose={onChoose} />}
      {tab === "phone" && <PhoneChoices onChoose={onChoose} />}
    </div>
  );
}

function ReceiverChoices({ onChoose }: { onChoose: Choose }) {
  const devices = useQuery(nmeaDevicesQuery());
  const [query, setQuery] = useState("");
  const listed = devices.data?.devices ?? [];
  const found = filterNmeaDevices(listed, query);

  return (
    <>
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
          {listed.length > 0 ? "No receiver matches that." : "No serial receiver found."}
        </p>
      )}
      {devices.isError && (
        <p role="alert" className="font-mono text-danger text-xs">
          Serial device discovery failed
        </p>
      )}

      <SerialPathForm onChoose={onChoose} />
    </>
  );
}

function PhoneChoices({ onChoose }: { onChoose: Choose }) {
  const phones = useQuery(phonesQuery());
  const listed = phones.data?.phones ?? [];
  return (
    <>
      <div className="flex flex-col gap-1">
        {listed.map((phone) => (
          <Button
            key={phone.id}
            type="button"
            className={`${BTN} justify-start`}
            onClick={() => onChoose({ type: "phone", phone: phone.id })}
          >
            <span
              aria-hidden
              className={`size-2 shrink-0 rounded-full ${phone.online ? "bg-ok" : "bg-ink-faint"}`}
            />
            <span className="min-w-0 flex-1 truncate text-left">{phone.name}</span>
            <span className="font-mono text-[10px] text-ink-faint">
              {STANDING_LABEL[phone.online ? "online" : "offline"]}
            </span>
          </Button>
        ))}
      </div>
      {phones.isSuccess && listed.length === 0 && (
        <p className="text-ink-dim text-sm" title="Pair one in Library > Phones">
          No phones
        </p>
      )}
      {phones.isError && (
        <p role="alert" className="font-mono text-danger text-xs">
          Phone list failed
        </p>
      )}
    </>
  );
}

function SerialPathForm({ onChoose }: { onChoose: Choose }) {
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
      <span className={LABEL}>Path</span>
      <Input
        className={`${FIELD} w-full`}
        type="text"
        aria-label="Serial device path"
        title="The serial port of a receiver that is not listed above"
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

function GpsdForm({ onChoose }: { onChoose: Choose }) {
  const [address, setAddress] = useState("127.0.0.1:2947");
  const valid = validGpsdAddress(address.trim());
  return (
    <Form
      className="flex items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (valid) {
          onChoose({ type: "gpsd", address: address.trim() });
        }
      }}
    >
      <span className={LABEL}>gpsd</span>
      <Input
        className={`${FIELD} w-full`}
        type="text"
        aria-label="GPSD address"
        title="Host and port of a gpsd daemon, on this machine or another one"
        placeholder="127.0.0.1:2947"
        value={address}
        onChange={(event) => setAddress(event.target.value)}
      />
      <Button type="submit" className={BTN} disabled={!valid}>
        Read
      </Button>
    </Form>
  );
}

function FixedForm({ onChoose }: { onChoose: Choose }) {
  const [lat, setLat] = useState(0);
  const [lon, setLon] = useState(0);
  return (
    <Form
      className="flex items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        onChoose({ type: "fixed", lat, lon });
      }}
    >
      <span className={LABEL}>Lat</span>
      <NumberField
        label="Latitude"
        unit="°"
        value={lat}
        min={-90}
        max={90}
        step={0.00001}
        onCommit={setLat}
        className="w-24"
      />
      <span className={LABEL}>Lon</span>
      <NumberField
        label="Longitude"
        unit="°"
        value={lon}
        min={-180}
        max={180}
        step={0.00001}
        onCommit={setLon}
        className="w-24"
      />
      <Button type="submit" className={BTN} title="Use these coordinates as the position">
        Set
      </Button>
    </Form>
  );
}
