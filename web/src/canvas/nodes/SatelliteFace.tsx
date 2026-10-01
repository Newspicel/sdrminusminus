import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN, BTN_QUIET, FIELD } from "../../components/controls";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { ChipField, Chips, SettingChip } from "../../components/face/Chips";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { FaceStats, Stat } from "../../components/face/Stats";
import { formatMhz } from "../../components/format";
import { SearchableSelect } from "../../components/SearchableSelect";
import { TuneTo } from "../../components/TuneTo";
import { TuningLock } from "../../components/TuningLock";
import { satellitesQuery, transmittersQuery } from "../../lib/api";
import { useSatelliteStore } from "../../lib/satellite";
import type {
  CatalogSatellite,
  PatchNode,
  PatchNodeOf,
  SatelliteNode,
  SatelliteStatus,
} from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";
import {
  compass,
  formatDoppler,
  passLine,
  pastedElements,
  SATELLITE_RANGE,
  STALE_ELEMENTS_DAYS,
  shownSignals,
  transmitterLabel,
} from "./satellite";

const SEARCH_DELAY_MS = 350;
const SHOWN_RESULTS = 8;

export function SatelliteFace({ node }: { node: PatchNode }) {
  if (node.kind !== "satellite") {
    return null;
  }
  return <SatelliteNodeFace node={node} />;
}

function SatelliteNodeFace({ node }: { node: PatchNodeOf<"satellite"> }) {
  const workspace = useWorkspaceContext();
  const status = useSatelliteStore((store) => store.byNode[node.id]) ?? null;
  const edit = (next: Partial<SatelliteNode>): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "satellite" ? { ...current, data: { ...current.data, ...next } } : current,
      ),
    }));
  const data = node.data;
  if (data.tle == null) {
    return (
      <NodeShell node={node} title="Satellite" category="tool">
        <FaceBody>
          <SatellitePicker onPick={(tle) => edit({ tle })} />
        </FaceBody>
      </NodeShell>
    );
  }
  return (
    <NodeShell
      node={node}
      title="Satellite"
      category="tool"
      subtitle={status?.name ?? status?.catalog ?? undefined}
    >
      <FaceBody>
        <Downlink
          node={node.id}
          hz={data.downlink_hz ?? null}
          held={data.transmitter != null}
          locked={data.tuning_locked ?? false}
          onLock={(tuning_locked) => edit({ tuning_locked })}
          onTune={(downlink_hz) => edit({ downlink_hz, uplink_hz: null, transmitter: null })}
        />
        <SignalChip catalog={status?.catalog ?? null} data={data} onEdit={edit} />
        <TrackReadout status={status} />
        {status?.error != null && <FaceFault message={status.error} />}
      </FaceBody>
      <FaceFooter>
        <ElementsAge status={status} />
        <RefreshElements catalog={status?.catalog ?? null} onFresh={(tle) => edit({ tle })} />
        <Button
          type="button"
          className={BTN_QUIET}
          onClick={() => edit({ tle: null, downlink_hz: null, uplink_hz: null, transmitter: null })}
        >
          Change satellite
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}

function useDebounced(value: string): string {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [value]);
  return settled;
}

function SatellitePicker({ onPick }: { onPick: (tle: string) => void }) {
  const [draft, setDraft] = useState("");
  const pasted = pastedElements(draft);
  const search = useDebounced(pasted === null ? draft.trim() : "");
  const results = useQuery({ ...satellitesQuery(search), enabled: pasted === null });
  const found: readonly CatalogSatellite[] = results.data?.satellites ?? [];
  return (
    <div className="flex flex-col gap-2 p-2">
      <Input
        aria-label="Search satellites"
        title="A name, a NORAD number, or pasted element lines"
        placeholder="ISS, 25544 or element lines"
        className={`${FIELD} w-full`}
        value={draft}
        onChange={(event) => setDraft(event.currentTarget.value)}
      />
      {pasted !== null ? (
        <Button type="button" className={BTN} onClick={() => onPick(pasted)}>
          Use these elements
        </Button>
      ) : results.isError ? (
        <p role="alert" className="text-xs text-danger">
          {results.error.message}
        </p>
      ) : (
        <ul className="flex flex-col">
          {found.slice(0, SHOWN_RESULTS).map((satellite) => (
            <li key={satellite.catalog}>
              <Button
                type="button"
                className={`${BTN_QUIET} w-full justify-between`}
                onClick={() => onPick(satellite.tle)}
              >
                <span className="truncate">{satellite.name}</span>
                <span className="legend">{satellite.catalog}</span>
              </Button>
            </li>
          ))}
          {results.isSuccess && found.length === 0 && (
            <li className="text-xs text-ink-dim">Nothing found</li>
          )}
        </ul>
      )}
    </div>
  );
}

function Downlink({
  node,
  hz,
  held,
  locked,
  onLock,
  onTune,
}: {
  node: string;
  hz: number | null;
  held: boolean;
  locked: boolean;
  onLock: (locked: boolean) => void;
  onTune: (hz: number) => void;
}) {
  const active = useFaceActive();
  return (
    <div
      className="@container flex min-w-0 items-center gap-1 border-b border-line p-2"
      title="Downlink before Doppler"
    >
      {hz === null ? (
        <span className="text-xs text-ink-dim">No downlink</span>
      ) : (
        <FrequencyDial
          id={dialId(node)}
          hz={hz}
          range={SATELLITE_RANGE}
          disabled={held || locked}
          wheelTunes={active}
          onTune={onTune}
        />
      )}
      <span className="ml-auto flex shrink-0 items-center gap-1">
        <TuneTo
          title="Type the downlink"
          hz={hz ?? 0}
          hint="Before Doppler"
          resolve={(entered) =>
            entered > SATELLITE_RANGE.min && entered <= SATELLITE_RANGE.max ? entered : null
          }
          disabled={held || locked}
          onTune={onTune}
        />
        <TuningLock
          locked={held || locked}
          held="Downlink locked"
          free="Lock downlink"
          hold={held ? "Set by the signal. Pick Own frequency to tune by hand." : null}
          onLock={onLock}
        />
      </span>
    </div>
  );
}

function SignalChip({
  catalog,
  data,
  onEdit,
}: {
  catalog: string | null;
  data: SatelliteNode;
  onEdit: (next: Partial<SatelliteNode>) => void;
}) {
  const listed = useQuery(transmittersQuery(catalog));
  const transmitters = shownSignals(listed.data?.transmitters ?? [], data.transmitter);
  if (transmitters.length === 0) {
    return null;
  }
  const picked = transmitters.find((transmitter) => transmitter.id === data.transmitter);
  return (
    <Chips className="p-2">
      <SettingChip
        label="Signal"
        value={picked === undefined ? "own" : transmitterLabel(picked)}
        quiet={picked === undefined}
        title={`What this satellite sends, from ${listed.data?.source ?? "SatNOGS DB"}`}
      >
        {() => (
          <ChipField label="Signal">
            <SearchableSelect
              label="Signal"
              className="min-w-0 flex-1"
              value={data.transmitter ?? ""}
              options={[
                { value: "", label: "Own frequency" },
                ...transmitters.map((transmitter) => ({
                  value: transmitter.id,
                  label: transmitterLabel(transmitter),
                })),
              ]}
              onChange={(id) => {
                const transmitter = transmitters.find((candidate) => candidate.id === id);
                onEdit(
                  transmitter === undefined
                    ? { transmitter: null }
                    : {
                        transmitter: transmitter.id,
                        downlink_hz: transmitter.downlink_hz ?? null,
                        uplink_hz: transmitter.uplink_hz ?? null,
                      },
                );
              }}
            />
          </ChipField>
        )}
      </SettingChip>
    </Chips>
  );
}

function useSecondsNow(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1_000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1_000)), 1_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

function TrackReadout({ status }: { status: SatelliteStatus | null }) {
  const now = useSecondsNow();
  const look = status?.look;
  if (status === null || (look == null && status.doppler_hz == null && status.uplink_hz == null)) {
    return null;
  }
  return (
    <Readouts>
      {look != null && (
        <Readout label="Look" tone={status.visible ? "ok" : undefined}>
          {look.azimuth_deg.toFixed(0)}° {compass(look.azimuth_deg)},{" "}
          {look.elevation_deg.toFixed(1)}° up
        </Readout>
      )}
      {look != null && <Readout label="Range">{look.range_km.toFixed(0)} km</Readout>}
      {status.doppler_hz != null && (
        <Readout label="Doppler" title="Added to every wired decoder">
          {formatDoppler(status.doppler_hz)}
          {status.doppler_rate_hz_s != null && `, ${formatDoppler(status.doppler_rate_hz_s)}/s`}
        </Readout>
      )}
      {status.uplink_hz != null && (
        <Readout
          label="Send on"
          title="The uplink corrected so the satellite hears it on frequency"
        >
          {formatMhz(status.uplink_hz)}
        </Readout>
      )}
      {look != null && <Readout label="Next pass">{passLine(status.next_pass, now)}</Readout>}
    </Readouts>
  );
}

function ElementsAge({ status }: { status: SatelliteStatus | null }) {
  const days = status?.tle_age_days;
  if (days == null) {
    return null;
  }
  return (
    <FaceStats>
      <Stat
        label="Elements"
        title="Age of the orbital elements. Older ones drift; refresh them"
        tone={days > STALE_ELEMENTS_DAYS ? "warn" : undefined}
      >
        {days.toFixed(1)} d
      </Stat>
    </FaceStats>
  );
}

function RefreshElements({
  catalog,
  onFresh,
}: {
  catalog: string | null;
  onFresh: (tle: string) => void;
}) {
  const fresh = useQuery({ ...satellitesQuery(catalog ?? ""), enabled: false });
  if (catalog === null) {
    return null;
  }
  return (
    <Button
      type="button"
      className={BTN}
      disabled={fresh.isFetching}
      title={fresh.isError ? fresh.error.message : "Fetch the newest elements"}
      onClick={() =>
        void fresh.refetch().then((result) => {
          const match = result.data?.satellites.find((satellite) => satellite.catalog === catalog);
          if (match !== undefined) {
            onFresh(match.tle);
          }
        })
      }
    >
      Refresh elements
    </Button>
  );
}
