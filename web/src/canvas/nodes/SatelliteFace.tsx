import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN, BTN_QUIET, FIELD } from "../../components/controls";
import { formatMhz } from "../../components/format";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
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
import { FaceBody, FaceFooter, NodeShell } from "./NodeShell";
import {
  compass,
  formatDoppler,
  passLine,
  pastedElements,
  STALE_ELEMENTS_DAYS,
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
      <NodeShell node={node} title="Satellite" category="tool" subtitle="none picked">
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
        <Settings className="p-2">
          <TransmitterRow catalog={status?.catalog ?? null} data={data} onEdit={edit} />
          <SettingRow label="Downlink" title="The published frequency; Doppler is added on top">
            <NumberField
              label="Downlink in MHz"
              value={(data.downlink_hz ?? 0) / 1e6}
              min={0}
              step={0.0001}
              onCommit={(mhz) => edit({ downlink_hz: mhz > 0 ? mhz * 1e6 : null })}
              className="w-32"
            />
            <span className="legend">MHz</span>
          </SettingRow>
          <SettingRow
            label="Min. elevation"
            title="A pass counts from this height over the horizon"
          >
            <NumberField
              label="Minimum elevation in degrees"
              value={data.min_elevation_deg ?? 0}
              min={-10}
              max={90}
              step={1}
              onCommit={(min_elevation_deg) => edit({ min_elevation_deg })}
              className="w-20"
            />
            <span className="legend">°</span>
          </SettingRow>
        </Settings>
        <TrackReadout status={status} />
      </FaceBody>
      <FaceFooter>
        <RefreshElements catalog={status?.catalog ?? null} onFresh={(tle) => edit({ tle })} />
        <Button
          type="button"
          className={BTN_QUIET}
          onClick={() => edit({ tle: null, downlink_hz: null, uplink_hz: null })}
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

function TransmitterRow({
  catalog,
  data,
  onEdit,
}: {
  catalog: string | null;
  data: SatelliteNode;
  onEdit: (next: Partial<SatelliteNode>) => void;
}) {
  const listed = useQuery(transmittersQuery(catalog));
  const transmitters = (listed.data?.transmitters ?? []).filter(
    (transmitter) => transmitter.downlink_hz != null,
  );
  if (transmitters.length === 0) {
    return null;
  }
  const chosen = transmitters.findIndex(
    (transmitter) => transmitter.downlink_hz === data.downlink_hz,
  );
  return (
    <SettingRow label="Transmitter" title={`From ${listed.data?.source ?? "SatNOGS DB"}`}>
      <Select
        label="Transmitter"
        value={chosen}
        options={[
          { value: -1, label: "Custom" },
          ...transmitters.map((transmitter, index) => ({
            value: index,
            label: transmitterLabel(transmitter),
          })),
        ]}
        onChange={(index) => {
          const transmitter = transmitters[index];
          if (transmitter !== undefined) {
            onEdit({
              downlink_hz: transmitter.downlink_hz ?? null,
              uplink_hz: transmitter.uplink_hz ?? null,
            });
          }
        }}
      />
    </SettingRow>
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
  if (status === null) {
    return null;
  }
  const look = status.look;
  const stale = (status.tle_age_days ?? 0) > STALE_ELEMENTS_DAYS;
  return (
    <Readout>
      {look != null && (
        <ReadoutRow label="Look">
          <span className={status.visible ? "text-accent" : ""}>
            {look.azimuth_deg.toFixed(0)}° {compass(look.azimuth_deg)},{" "}
            {look.elevation_deg.toFixed(1)}° up
          </span>
        </ReadoutRow>
      )}
      {look != null && <ReadoutRow label="Range">{look.range_km.toFixed(0)} km</ReadoutRow>}
      {status.doppler_hz != null && (
        <ReadoutRow label="Doppler" title="Added to every wired decoder">
          {formatDoppler(status.doppler_hz)}
          {status.doppler_rate_hz_s != null && `, ${formatDoppler(status.doppler_rate_hz_s)}/s`}
        </ReadoutRow>
      )}
      {status.uplink_hz != null && (
        <ReadoutRow
          label="Send on"
          title="The uplink corrected so the satellite hears it on frequency"
        >
          {formatMhz(status.uplink_hz)}
        </ReadoutRow>
      )}
      {look != null && <ReadoutRow label="Next pass">{passLine(status.next_pass, now)}</ReadoutRow>}
      {status.tle_age_days != null && (
        <ReadoutRow label="Elements" title="Older elements drift; refresh them">
          <span className={stale ? "text-warn" : ""}>
            {status.tle_age_days.toFixed(1)} days old
          </span>
        </ReadoutRow>
      )}
      {status.error != null && (
        <ReadoutRow label="Fault">
          <span role="alert" className="text-danger">
            {status.error}
          </span>
        </ReadoutRow>
      )}
    </Readout>
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
