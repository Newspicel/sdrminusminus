import { useQuery } from "@tanstack/react-query";
import {
  Fragment,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { capturedImageUrl, imagesQuery } from "../lib/api";
import { useDecodedKind, useDecodedStore, useStations } from "../lib/decoded";
import type { DecodedRecordOf, DecoderKind } from "../lib/types";
import { Button } from "./BaseControls";
import { ALERT, BTN, CHIP, TABLE_CELL, TABLE_HEAD } from "./controls";
import {
  ageClass,
  aircraftRow,
  buildTranscript,
  candidateScore,
  cwSignalRows,
  type DecoderScope,
  type DectStation,
  dectStations,
  formatAge,
  formatAltFreqs,
  formatClock,
  identMeasurements,
  inScope,
  isAtBottom,
  latestVorReadings,
  latestWpm,
  modulationLabel,
  multiVorFix,
  ptyLabel,
  rdsPicture,
  rdsQuality,
  recordsInScope,
  shipRow,
  sortTargets,
  stationsInScope,
  TARGET_MAX_AGE_MS,
  type TargetRow,
  type TargetSort,
  toneLabel,
} from "./decoderViews";

const PANE = "flex flex-col gap-2 p-3";
const EMPTY = "text-sm text-ink-dim";

function useNow(periodMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), periodMs);
    return () => clearInterval(id);
  }, [periodMs]);
  return now;
}

function RdsView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("rds"), scope);
  const rds = rdsPicture(records);

  if (rds === null) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No RDS yet — tune a WFM channel to a station that carries it.</span>
      </div>
    );
  }

  const quality = rdsQuality(rds);
  const altFreqs = formatAltFreqs(rds.alt_freqs_hz);

  return (
    <div className={PANE}>
      <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
        <span className="font-mono text-2xl tracking-wide text-ink">
          {rds.ps?.trim() || "········"}
        </span>
        <span className="font-mono text-sm tabular-nums text-ink-dim">PI {rds.pi ?? "—"}</span>
        <span className="text-sm text-ink-dim">{ptyLabel(rds)}</span>
        <div className="ml-auto flex items-center gap-1">
          <Flag label="TP" on={rds.tp === true} />
          <Flag label="TA" on={rds.ta === true} />
          <Flag label={rds.music === false ? "SP" : "MS"} on={rds.music != null} />
        </div>
      </div>

      <div>
        <div className="legend">RadioText</div>
        <div className="overflow-x-auto whitespace-nowrap rounded border border-line bg-panel px-2 py-1.5 font-mono text-sm text-ink">
          {rds.radiotext?.trim() || <span className="text-ink-dim">—</span>}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
        <div className="flex flex-wrap items-center gap-1">
          <span className="legend">AF</span>
          {altFreqs.length === 0 ? (
            <span className="font-mono text-xs text-ink-dim">—</span>
          ) : (
            altFreqs.map((af) => (
              <span key={af} className={CHIP}>
                {af}
              </span>
            ))
          )}
        </div>
        <div className="ml-auto flex items-center gap-2 font-mono text-xs tabular-nums text-ink-dim">
          <span className="legend">Quality</span>
          <span className={quality.className}>{quality.label}</span>
          <span>
            {quality.groups} groups · {quality.blockErrors} block errors ·{" "}
            {(quality.errorRate * 100).toFixed(1)}%
          </span>
        </div>
      </div>
    </div>
  );
}

function Flag({ label, on }: { label: string; on: boolean }) {
  return (
    <span
      className={`rounded border px-1.5 py-0.5 font-mono text-xs ${
        on ? "border-accent text-accent" : "border-line text-ink-dim opacity-50"
      }`}
    >
      {label}
    </span>
  );
}

const TARGET_COLUMNS = {
  adsb: {
    title: "Aircraft",
    idHeader: "ICAO",
    labelHeader: "Callsign",
    primaryHeader: "Altitude",
    secondaryHeader: "Speed / track",
  },
  ais: {
    title: "Ships",
    idHeader: "MMSI",
    labelHeader: "Name",
    primaryHeader: "Speed",
    secondaryHeader: "Course / destination",
  },
} as const;

function TargetsView({ kind, scope = {} }: { kind: "adsb" | "ais"; scope?: DecoderScope }) {
  const now = useNow();
  const ageOut = useDecodedStoreAgeOut();
  const aircraft = stationsInScope(useStations("adsb"), scope);
  const ships = stationsInScope(useStations("ais"), scope);
  const [sort, setSort] = useState<TargetSort>("age");
  const [descending, setDescending] = useState(false);

  useEffect(() => ageOut(now), [ageOut, now]);

  const toggle = (key: TargetSort): void => {
    if (key === sort) {
      setDescending(!descending);
    } else {
      setSort(key);
      setDescending(false);
    }
  };

  const rows =
    kind === "adsb" ? aircraft.map((s) => aircraftRow(s, now)) : ships.map((s) => shipRow(s, now));

  return (
    <div className={PANE}>
      <TargetTable
        {...TARGET_COLUMNS[kind]}
        rows={sortTargets(rows, sort, descending)}
        sort={sort}
        descending={descending}
        onSort={toggle}
      />
    </div>
  );
}

function TargetTable({
  title,
  idHeader,
  labelHeader,
  primaryHeader,
  secondaryHeader,
  rows,
  sort,
  descending,
  onSort,
}: {
  title: string;
  idHeader: string;
  labelHeader: string;
  primaryHeader: string;
  secondaryHeader: string;
  rows: readonly TargetRow[];
  sort: TargetSort;
  descending: boolean;
  onSort: (key: TargetSort) => void;
}) {
  const arrow = (key: TargetSort): string => (sort !== key ? "" : descending ? " ↓" : " ↑");
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex items-baseline gap-2">
        <span className="legend">{title}</span>
        <span className="font-mono text-[10px] tabular-nums text-ink-dim">{rows.length}</span>
      </div>
      {rows.length === 0 ? (
        <span className={EMPTY}>No {title.toLowerCase()} heard.</span>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[32rem] border-collapse">
            <thead>
              <tr className="border-b border-line">
                <th className={TABLE_HEAD} scope="col">
                  <SortButton label={idHeader + arrow("id")} onClick={() => onSort("id")} />
                </th>
                <th className={TABLE_HEAD} scope="col">
                  {labelHeader}
                </th>
                <th className={TABLE_HEAD} scope="col">
                  {primaryHeader}
                </th>
                <th className={TABLE_HEAD} scope="col">
                  {secondaryHeader}
                </th>
                <th className={TABLE_HEAD} scope="col">
                  Position
                </th>
                <th className={TABLE_HEAD} scope="col">
                  <SortButton label={`Age${arrow("age")}`} onClick={() => onSort("age")} />
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.id} className={`border-b border-line/50 ${ageClass(row.ageMs)}`}>
                  <td className={`${TABLE_CELL} font-semibold`}>{row.id}</td>
                  <td className={TABLE_CELL}>{row.label}</td>
                  <td className={TABLE_CELL}>{row.primary}</td>
                  <td className={TABLE_CELL}>{row.secondary || "—"}</td>
                  <td className={TABLE_CELL}>{row.position}</td>
                  <td className={`${TABLE_CELL} text-right`}>{formatAge(row.ageMs)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function SortButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <Button type="button" className="uppercase tracking-wider hover:text-accent" onClick={onClick}>
      {label}
    </Button>
  );
}

function TextView({ kind, scope = {} }: { kind: "rtty" | "morse" | "psk"; scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind(kind), scope);
  const text = buildTranscript(records);
  const wpm = kind === "morse" ? latestWpm(records as readonly DecodedRecordOf<"morse">[]) : null;
  const paneRef = useRef<HTMLPreElement>(null);
  const stick = useRef(true);
  const [copyError, setCopyError] = useState<string | null>(null);

  useLayoutEffect(() => {
    const el = paneRef.current;
    if (el !== null && stick.current) {
      el.scrollTop = el.scrollHeight;
    }
    // oxlint-disable-next-line react/exhaustive-effect-dependencies -- new text is what scrolls the pane
  }, [text]);

  const copy = (): void => {
    void (async () => {
      try {
        await navigator.clipboard.writeText(text);
        setCopyError(null);
      } catch (e) {
        setCopyError(e instanceof Error ? e.message : String(e));
      }
    })();
  };

  return (
    <div className={PANE}>
      <div className="flex items-center gap-3">
        <span className="legend">{kindLabel(kind)}</span>
        {wpm !== null && (
          <span className="font-mono text-xs tabular-nums text-ink">
            {wpm.toFixed(0)} <span className="text-ink-dim">WPM</span>
          </span>
        )}
        <Button type="button" className={`${BTN} ml-auto`} disabled={text === ""} onClick={copy}>
          Copy all
        </Button>
      </div>

      {copyError !== null && (
        <div role="alert" className={`${ALERT} flex items-center justify-between gap-3`}>
          <span>Copy failed: {copyError}</span>
          <Button type="button" className="shrink-0 underline" onClick={() => setCopyError(null)}>
            dismiss
          </Button>
        </div>
      )}

      <pre
        ref={paneRef}
        tabIndex={0}
        aria-label={`${kind} transcript`}
        className="max-h-72 min-h-32 flex-1 overflow-auto whitespace-pre-wrap break-words rounded border border-line bg-panel px-2 py-1.5 font-mono text-xs text-ink"
        onScroll={(e) => {
          stick.current = isAtBottom(e.currentTarget);
        }}
      >
        {text}
      </pre>
    </div>
  );
}

function CwSkimmerView({ scope = {} }: { scope?: DecoderScope }) {
  const rows = cwSignalRows(recordsInScope(useDecodedKind("cw_skimmer"), scope));
  return (
    <div className={PANE}>
      <div className="flex items-baseline gap-2">
        <span className="legend">Signals in passband</span>
        <span className="font-mono text-xs text-ink-dim">{rows.length}</span>
      </div>
      {rows.length === 0 ? (
        <span className={EMPTY}>No CW carriers decoded yet.</span>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[34rem] border-collapse">
            <thead>
              <tr className="border-b border-line">
                <th className={TABLE_HEAD}>Frequency</th>
                <th className={TABLE_HEAD}>Offset</th>
                <th className={TABLE_HEAD}>Speed</th>
                <th className={TABLE_HEAD}>SNR</th>
                <th className={TABLE_HEAD}>Text</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={Math.round(row.offsetHz)} className="border-b border-line/50">
                  <td className={`${TABLE_CELL} tabular-nums`}>
                    {(row.frequencyHz / 1e6).toFixed(6)} MHz
                  </td>
                  <td className={`${TABLE_CELL} tabular-nums`}>
                    {row.offsetHz >= 0 ? "+" : ""}
                    {row.offsetHz.toFixed(0)} Hz
                  </td>
                  <td className={`${TABLE_CELL} tabular-nums`}>{row.wpm.toFixed(0)} WPM</td>
                  <td className={`${TABLE_CELL} tabular-nums`}>{row.snrDb.toFixed(0)} dB</td>
                  <td
                    className={`${TABLE_CELL} max-w-[24rem] whitespace-pre-wrap break-words font-mono text-ink`}
                  >
                    {row.text}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function kindLabel(kind: "rtty" | "morse" | "psk"): string {
  return { rtty: "RTTY", morse: "Morse", psk: "PSK" }[kind];
}

function useDecodedStoreAgeOut(): (nowMs: number) => void {
  const ageOut = useDecodedStore((s) => s.ageOut);
  return useCallback((nowMs: number) => ageOut(TARGET_MAX_AGE_MS, nowMs), [ageOut]);
}

function ToneView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("tone"), scope);
  const latest = records[0];

  if (latest === undefined) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No subaudible tone heard.</span>
      </div>
    );
  }

  const status = latest.event.data;
  const label = toneLabel(status);
  return (
    <div className={PANE}>
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="font-mono text-xs tabular-nums text-ink-dim">
          {formatClock(latest.at)}
        </span>
        <span className="font-mono text-xs tabular-nums text-accent">
          {label === "" ? "no tone" : label}
        </span>
        <span className="legend">{status.open ? "open" : "muted"}</span>
      </div>
    </div>
  );
}

function IdentView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("ident"), scope);
  const latest = records[0];

  if (latest === undefined) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>
          Nothing analysed yet — point the channel at a signal and wait one report interval.
        </span>
      </div>
    );
  }

  const report = latest.event.data;
  const candidates = report.candidates ?? [];

  return (
    <div className={PANE}>
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="font-mono text-2xl tracking-wide text-ink">{modulationLabel(report)}</span>
        {report.modulation !== "none" && (
          <span className="legend">{Math.round(report.confidence * 100)}% confident</span>
        )}
        <span className="ml-auto font-mono text-xs tabular-nums text-ink-dim">
          {formatClock(latest.at)}
        </span>
      </div>

      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
        {identMeasurements(report).map(([label, value]) => (
          <Fragment key={label}>
            <dt className="legend self-center">{label}</dt>
            <dd className="font-mono text-xs tabular-nums text-ink">{value}</dd>
          </Fragment>
        ))}
      </dl>

      <div>
        <div className="legend">Protocol</div>
        {candidates.length === 0 ? (
          <span className={EMPTY}>Nothing in the catalog fits these measurements.</span>
        ) : (
          <ul className="flex flex-col gap-1">
            {candidates.map((match) => (
              <li key={match.name} className="flex flex-wrap items-baseline gap-2">
                <span
                  className={`font-mono text-sm ${match.confirmed === true ? "text-accent" : "text-ink"}`}
                >
                  {match.name}
                </span>
                <span className={CHIP}>{candidateScore(match)}</span>
                <span className="text-xs text-ink-dim">{match.why}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

const DECT_CIPHER_CHIP: Record<string, string> = {
  clear: "no encryption seen",
  requested: "start requested",
  confirmed: "start confirmed",
  active: "encrypted",
  stopped: "encryption stopped",
};

function dectSupport(value: boolean | null): string {
  return value === null ? "—" : value ? "yes" : "no";
}

function DectRow({ station }: { station: DectStation }) {
  return (
    <tr>
      <td className={`${TABLE_CELL} font-mono`}>{station.rfpi ?? "—"}</td>
      <td className={TABLE_CELL}>{station.arc === null ? "—" : station.arc.toUpperCase()}</td>
      <td className={TABLE_CELL}>
        {station.carrier === null
          ? "—"
          : station.carrierHz === null
            ? String(station.carrier)
            : `${station.carrier} · ${(station.carrierHz / 1e6).toFixed(3)} MHz`}
      </td>
      <td className={TABLE_CELL}>{station.slotPair === null ? "—" : String(station.slotPair)}</td>
      <td className={TABLE_CELL}>{dectSupport(station.authentication)}</td>
      <td className={TABLE_CELL}>{dectSupport(station.ciphering)}</td>
      <td className={TABLE_CELL}>
        <span className={CHIP}>{DECT_CIPHER_CHIP[station.cipherState] ?? station.cipherState}</span>
      </td>
      <td className={TABLE_CELL}>{station.handsets === 0 ? "—" : String(station.handsets)}</td>
      <td className={TABLE_CELL}>{station.levelDbfs.toFixed(1)}</td>
      <td className={TABLE_CELL}>
        {station.bursts}
        {station.crcErrors === 0 ? "" : ` / ${station.crcErrors} bad`}
      </td>
    </tr>
  );
}

function DectView({ scope = {} }: { scope?: DecoderScope }) {
  const stations = dectStations(recordsInScope(useDecodedKind("dect"), scope));
  if (stations.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No DECT base stations heard yet.</span>
      </div>
    );
  }
  return (
    <div className={PANE}>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left text-xs">
          <thead>
            <tr>
              <th className={TABLE_HEAD}>RFPI</th>
              <th className={TABLE_HEAD}>Class</th>
              <th className={TABLE_HEAD}>Carrier</th>
              <th className={TABLE_HEAD}>Slot</th>
              <th className={TABLE_HEAD}>Auth</th>
              <th className={TABLE_HEAD}>Cipher</th>
              <th className={TABLE_HEAD}>State</th>
              <th className={TABLE_HEAD}>Handsets</th>
              <th className={TABLE_HEAD}>dBFS</th>
              <th className={TABLE_HEAD}>Bursts</th>
            </tr>
          </thead>
          <tbody>
            {stations.map((station) => (
              <DectRow key={station.key} station={station} />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function VorView({ scope = {} }: { scope?: DecoderScope }) {
  const readings = latestVorReadings(recordsInScope(useDecodedKind("vor"), scope));
  const fix = multiVorFix(readings);
  if (readings.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No VOR reports yet.</span>
      </div>
    );
  }
  return (
    <div className={PANE}>
      {fix === null ? (
        <span className={EMPTY}>
          Add coordinates to two non-parallel VOR channels to calculate a position fix.
        </span>
      ) : (
        <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
          <span className="font-mono text-xl tabular-nums text-ink">
            {fix.lat.toFixed(5)}, {fix.lon.toFixed(5)}
          </span>
          <span className={CHIP}>{fix.stations} stations</span>
          <span className="text-xs text-ink-dim">
            {fix.residualKm < 1
              ? `${Math.round(fix.residualKm * 1000)} m`
              : `${fix.residualKm.toFixed(1)} km`}{" "}
            residual
          </span>
        </div>
      )}
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left text-xs">
          <thead>
            <tr>
              <th className={TABLE_HEAD}>Station</th>
              <th className={TABLE_HEAD}>Radial</th>
              <th className={TABLE_HEAD}>Confidence</th>
              <th className={TABLE_HEAD}>Signal</th>
            </tr>
          </thead>
          <tbody>
            {readings.map((record) => {
              const reading = record.event.data;
              return (
                <tr key={reading.station ?? `${record.device_set}:${record.channel}`}>
                  <td className={TABLE_CELL}>
                    {reading.station ?? `D${record.device_set} C${record.channel}`}
                  </td>
                  <td className={`${TABLE_CELL} font-mono tabular-nums`}>
                    {reading.radial_deg.toFixed(1)}°
                  </td>
                  <td className={`${TABLE_CELL} font-mono tabular-nums`}>
                    {Math.round(reading.confidence * 100)}%
                  </td>
                  <td className={`${TABLE_CELL} font-mono tabular-nums`}>
                    {reading.signal_db.toFixed(1)} dB
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function PicturesView({ scope = {} }: { scope?: DecoderScope }) {
  const result = useQuery(imagesQuery());
  const images = (result.data?.images ?? []).filter((image) =>
    inScope(image.device_set, image.channel, scope),
  );
  const [selected, setSelected] = useState<number | null>(null);
  const open = images.find((image) => image.id === selected) ?? images[0];

  if (images.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>
          No picture received yet — a scanning transmission takes between 36 s and four minutes.
        </span>
      </div>
    );
  }

  return (
    <div className={PANE}>
      {open !== undefined && (
        <figure className="flex flex-col gap-1">
          {open.image == null ? (
            <span className={ALERT}>{open.image_error ?? "the pixels were not kept"}</span>
          ) : (
            <img
              src={capturedImageUrl(open.image.url)}
              alt={`${open.mode} picture received at ${formatClock(open.at)}`}
              className="w-full self-start rounded border border-line bg-black object-contain"
              style={{ maxHeight: "50vh" }}
            />
          )}
          <figcaption className="flex flex-wrap items-baseline gap-2">
            <span className="font-mono text-xs tabular-nums text-accent">{open.mode}</span>
            <span className="legend">
              {open.width}&#215;{open.height}
            </span>
            <span className="legend">
              {open.complete ? "complete" : `${open.lines} of ${open.height} lines`}
            </span>
            <span className="ml-auto font-mono text-xs tabular-nums text-ink-dim">
              {formatClock(open.at)}
            </span>
          </figcaption>
        </figure>
      )}
      {images.length > 1 && (
        <ul className="flex flex-wrap gap-2">
          {images.map((image) => (
            <li key={image.id}>
              <Button
                type="button"
                className={`${BTN} p-0.5`}
                aria-current={image.id === open?.id}
                aria-label={`${image.mode} at ${formatClock(image.at)}`}
                onClick={() => setSelected(image.id)}
              >
                {image.image == null ? (
                  <span className="legend px-2">no pixels</span>
                ) : (
                  <img
                    src={capturedImageUrl(image.image.url)}
                    alt=""
                    className="h-12 w-16 rounded-xs bg-black object-contain"
                  />
                )}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

const VIEWS: Record<DecoderKind, ((scope: DecoderScope) => ReactNode) | null> = {
  call: null,
  scrambler: null,
  rds: (scope) => <RdsView scope={scope} />,
  adsb: (scope) => <TargetsView kind="adsb" scope={scope} />,
  ais: (scope) => <TargetsView kind="ais" scope={scope} />,
  rtty: (scope) => <TextView kind="rtty" scope={scope} />,
  morse: (scope) => <TextView kind="morse" scope={scope} />,
  cw_skimmer: (scope) => <CwSkimmerView scope={scope} />,
  psk: (scope) => <TextView kind="psk" scope={scope} />,
  selcall: null,
  tone: (scope) => <ToneView scope={scope} />,
  ident: (scope) => <IdentView scope={scope} />,
  aprs: null,
  pocsag: null,
  flex: null,
  ermes: null,
  navtex: null,
  acars: null,
  subghz: null,
  dv: null,
  ft8: null,
  ft4: null,
  wspr: null,
  broadcast: null,
  radio_clock: null,
  gnss: null,
  sstv: (scope) => <PicturesView scope={scope} />,
  vor: (scope) => <VorView scope={scope} />,
  ils: null,
  df: null,
  df_fix: null,
  radar: null,
  dsc: null,
  inmarsat_stdc: null,
  inmarsat_aero: null,
  vdl2: null,
  hfdl: null,
  iridium: null,
  dect: (scope) => <DectView scope={scope} />,
};

function isDecoderKind(kind: string): kind is DecoderKind {
  return Object.hasOwn(VIEWS, kind);
}

export function hasDecoderView(kind: string): boolean {
  return isDecoderKind(kind) && VIEWS[kind] !== null;
}

export function DecoderView({ kind, scope }: { kind: string; scope: DecoderScope }) {
  return isDecoderKind(kind) ? (VIEWS[kind]?.(scope) ?? null) : null;
}
