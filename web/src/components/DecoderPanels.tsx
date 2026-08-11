// Per-decoder live views (PLAN §13). All of them read the decoded store, never TanStack Query:
// decoder frames are a stream, not server state. The projection/format/sort logic lives in
// `decoderViews.ts`; these components only render it.
//
// A view is the lower half of a channel node's face (CANVAS §8 phase ③), so each one is scoped
// to a single channel and `DecoderView` picks the one that channel's `decoder_kind` calls for.
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useDecodedKind, useDecodedStore, useStations } from "../lib/decoded";
import type { DecodedRecordOf, DecoderKind } from "../lib/types";
import { BTN, FIELD } from "./controls";
import {
  acarsHeadline,
  acarsTag,
  ageClass,
  aircraftRow,
  aprsMotion,
  buildTranscript,
  type DecoderScope,
  dvKind,
  dvMode,
  dvNetwork,
  dvParties,
  formatAge,
  formatAltFreqs,
  formatClock,
  formatPosition,
  formatRic,
  functionLabel,
  isAtBottom,
  latestWpm,
  matchesAddress,
  navtexHeader,
  navtexQuality,
  ptyLabel,
  rdsPicture,
  rdsQuality,
  recordsInScope,
  shipRow,
  sortTargets,
  stationsInScope,
  subghzPayload,
  subghzReadings,
  subghzTiming,
  TARGET_MAX_AGE_MS,
  type TargetRow,
  type TargetSort,
} from "./decoderViews";

const PANE = "flex flex-col gap-2 p-3";
const CAPTION = "text-[10px] font-semibold uppercase tracking-wider text-ink-dim";
const EMPTY = "text-sm text-ink-dim";
const TH = "px-2 py-1 text-left text-[10px] font-semibold uppercase tracking-wider text-ink-dim";
const TD = "px-2 py-1 font-mono text-xs tabular-nums";

/** Targets and pager/APRS ages are wall-clock relative, so the views need a clock. One shared
 * 1 Hz tick drives every age column — this is a re-render of local state, not a refetch. */
function useNow(periodMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), periodMs);
    return () => clearInterval(id);
  }, [periodMs]);
  return now;
}

export function RdsView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("rds"), scope);
  const rds = rdsPicture(records);

  if (rds === null) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No RDS yet — tune a WFM channel with RDS enabled.</span>
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
        <div className={CAPTION}>RadioText</div>
        <div className="overflow-x-auto whitespace-nowrap rounded border border-line bg-panel px-2 py-1.5 font-mono text-sm text-ink">
          {rds.radiotext?.trim() || <span className="text-ink-dim">—</span>}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
        <div className="flex flex-wrap items-center gap-1">
          <span className={CAPTION}>AF</span>
          {altFreqs.length === 0 ? (
            <span className="font-mono text-xs text-ink-dim">—</span>
          ) : (
            altFreqs.map((af) => (
              <span
                key={af}
                className="rounded border border-line px-1.5 py-0.5 font-mono text-xs tabular-nums text-ink-dim"
              >
                {af}
              </span>
            ))
          )}
        </div>
        <div className="ml-auto flex items-center gap-2 font-mono text-xs tabular-nums text-ink-dim">
          <span className={CAPTION}>Quality</span>
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

export function TargetsView({ kind, scope = {} }: { kind: "adsb" | "ais"; scope?: DecoderScope }) {
  const now = useNow();
  const ageOut = useDecodedStoreAgeOut();
  // Both stores are read unconditionally: reading one behind a test on `kind` would change the
  // hook count if a channel's type were patched in place, which React answers by tearing the
  // tree down (same rule as `TextView`).
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
        <span className={CAPTION}>{title}</span>
        <span className="font-mono text-[10px] tabular-nums text-ink-dim">{rows.length}</span>
      </div>
      {rows.length === 0 ? (
        <span className={EMPTY}>No {title.toLowerCase()} heard.</span>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[32rem] border-collapse">
            <thead>
              <tr className="border-b border-line">
                <th className={TH} scope="col">
                  <SortButton label={idHeader + arrow("id")} onClick={() => onSort("id")} />
                </th>
                <th className={TH} scope="col">
                  {labelHeader}
                </th>
                <th className={TH} scope="col">
                  {primaryHeader}
                </th>
                <th className={TH} scope="col">
                  {secondaryHeader}
                </th>
                <th className={TH} scope="col">
                  Position
                </th>
                <th className={TH} scope="col">
                  <SortButton label={`Age${arrow("age")}`} onClick={() => onSort("age")} />
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.id} className={`border-b border-line/50 ${ageClass(row.ageMs)}`}>
                  <td className={`${TD} font-semibold`}>{row.id}</td>
                  <td className={TD}>{row.label}</td>
                  <td className={TD}>{row.primary}</td>
                  <td className={TD}>{row.secondary || "—"}</td>
                  <td className={TD}>{row.position}</td>
                  <td className={`${TD} text-right`}>{formatAge(row.ageMs)}</td>
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
    <button type="button" className="uppercase tracking-wider hover:text-accent" onClick={onClick}>
      {label}
    </button>
  );
}

export function TextView({ kind, scope = {} }: { kind: "rtty" | "morse"; scope?: DecoderScope }) {
  // Both kinds go through the one `useDecodedKind(kind)` call: reading morse a second time
  // behind a `kind === "morse"` test would change the hook count when a channel's type is
  // patched from rtty to morse in place, which React answers by tearing down the tree.
  const records = recordsInScope(useDecodedKind(kind), scope);
  const text = buildTranscript(records);
  const wpm = kind === "morse" ? latestWpm(records as readonly DecodedRecordOf<"morse">[]) : null;
  const paneRef = useRef<HTMLPreElement>(null);
  // Stickiness is sampled on scroll, before the DOM grows: once the text has been appended the
  // old scroll position can no longer tell us whether the user had been reading the tail.
  const stick = useRef(true);
  const [copyError, setCopyError] = useState<string | null>(null);

  useLayoutEffect(() => {
    const el = paneRef.current;
    if (el !== null && stick.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [text]);

  // `navigator.clipboard` is absent outside a secure context, so the failure has to surface as a
  // banner rather than an unhandled rejection (CLAUDE.md: no silent failure).
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
        <span className={CAPTION}>{kind === "morse" ? "Morse" : "RTTY"}</span>
        {wpm !== null && (
          <span className="font-mono text-xs tabular-nums text-ink">
            {wpm.toFixed(0)} <span className="text-ink-dim">WPM</span>
          </span>
        )}
        <button type="button" className={`${BTN} ml-auto`} disabled={text === ""} onClick={copy}>
          Copy all
        </button>
      </div>

      {copyError !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Copy failed: {copyError}</span>
          <button type="button" className="shrink-0 underline" onClick={() => setCopyError(null)}>
            dismiss
          </button>
        </div>
      )}

      <pre
        ref={paneRef}
        // The transcript is a live tail the user can scroll back through; `tabIndex` keeps that
        // reachable from the keyboard.
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

export function PagerView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("pocsag"), scope);
  const [filter, setFilter] = useState("");
  const shown = records.filter((r) => matchesAddress(r.event.data.address, filter));

  return (
    <div className={PANE}>
      <label className="flex items-center gap-2 text-sm text-ink-dim">
        RIC
        <input
          type="text"
          inputMode="numeric"
          className={`${FIELD} w-28 tabular-nums`}
          value={filter}
          placeholder="all"
          onChange={(e) => setFilter(e.target.value)}
          aria-label="Filter by RIC address"
        />
        <span className="font-mono text-xs tabular-nums">
          {shown.length}/{records.length}
        </span>
      </label>

      {shown.length === 0 ? (
        <span className={EMPTY}>No pager messages.</span>
      ) : (
        <div className="flex flex-col divide-y divide-line">
          {shown.map((r, i) => (
            <div key={`${r.at}-${r.event.data.address}-${i}`} className="flex gap-2 py-1">
              <span className="font-mono text-xs tabular-nums text-ink-dim">
                {formatClock(r.at)}
              </span>
              <span className="font-mono text-xs tabular-nums text-accent">
                {formatRic(r.event.data.address)}
              </span>
              <span className="font-mono text-xs text-ink-dim">
                {functionLabel(r.event.data.function)}
              </span>
              <span className="min-w-0 flex-1 break-words font-mono text-xs text-ink">
                {r.event.data.text || (
                  <span className="text-ink-dim">({r.event.data.payload})</span>
                )}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function AprsView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("aprs"), scope);

  if (records.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No APRS packets.</span>
      </div>
    );
  }

  return (
    <div className={PANE}>
      <div className="flex flex-col divide-y divide-line">
        {records.map((r, i) => {
          const p = r.event.data;
          const motion = aprsMotion(p);
          const position = formatPosition(p.lat, p.lon);
          return (
            <div key={`${r.at}-${p.source}-${i}`} className="flex flex-col gap-0.5 py-1">
              <div className="flex gap-2">
                <span className="font-mono text-xs tabular-nums text-ink-dim">
                  {formatClock(r.at)}
                </span>
                <span className="min-w-0 flex-1 break-all font-mono text-xs text-ink">
                  {p.tnc2}
                </span>
              </div>
              {(position !== "—" || motion !== "") && (
                <div className="pl-14 font-mono text-[10px] tabular-nums text-ink-dim">
                  {position !== "—" && <span>{position}</span>}
                  {position !== "—" && motion !== "" && <span> · </span>}
                  {motion !== "" && <span>{motion}</span>}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Stable `ageOut` binding: the targets view drives the store's horizon so a target that stopped
 * transmitting eventually leaves the table instead of dimming forever. */
function useDecodedStoreAgeOut(): (nowMs: number) => void {
  const ageOut = useDecodedStore((s) => s.ageOut);
  return useCallback((nowMs: number) => ageOut(TARGET_MAX_AGE_MS, nowMs), [ageOut]);
}

export function NavtexView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("navtex"), scope);

  if (records.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No NAVTEX broadcasts — a station may be idle for minutes.</span>
      </div>
    );
  }

  return (
    <div className={PANE}>
      <div className="flex flex-col divide-y divide-line">
        {records.map((r, i) => {
          const m = r.event.data;
          const header = navtexHeader(m);
          const quality = navtexQuality(m);
          return (
            <article key={`${r.at}-${header ?? ""}-${i}`} className="flex flex-col gap-1 py-2">
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="font-mono text-xs tabular-nums text-ink-dim">
                  {formatClock(r.at)}
                </span>
                <span className="font-mono text-xs tabular-nums text-accent">{header ?? "—"}</span>
                {m.subject_name != null && (
                  <span className="text-xs text-ink-dim">{m.subject_name}</span>
                )}
                {quality !== "" && (
                  <span className="font-mono text-[10px] tabular-nums text-danger">{quality}</span>
                )}
              </div>
              <pre className="whitespace-pre-wrap break-words font-mono text-xs text-ink">
                {m.text}
              </pre>
            </article>
          );
        })}
      </div>
    </div>
  );
}

export function AcarsView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("acars"), scope);

  if (records.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No ACARS blocks.</span>
      </div>
    );
  }

  return (
    <div className={PANE}>
      <div className="flex flex-col divide-y divide-line">
        {records.map((r, i) => {
          const m = r.event.data;
          return (
            <article
              key={`${r.at}-${m.registration}-${m.seq_no ?? ""}-${i}`}
              className="flex flex-col gap-0.5 py-1"
            >
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="font-mono text-xs tabular-nums text-ink-dim">
                  {formatClock(r.at)}
                </span>
                <span className="font-mono text-xs text-accent">{acarsHeadline(m)}</span>
                <span className="font-mono text-[10px] tabular-nums text-ink-dim">
                  {acarsTag(m)}
                </span>
              </div>
              {m.text !== "" && (
                <pre className="whitespace-pre-wrap break-all pl-14 font-mono text-xs text-ink">
                  {m.text}
                </pre>
              )}
            </article>
          );
        })}
      </div>
    </div>
  );
}

export function SubghzView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("subghz"), scope);

  if (records.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No sub-GHz bursts — press a remote.</span>
      </div>
    );
  }

  return (
    <div className={PANE}>
      <div className="flex flex-col divide-y divide-line">
        {records.map((r, i) => {
          const f = r.event.data;
          const readings = subghzReadings(f);
          return (
            <div key={`${r.at}-${f.data}-${i}`} className="flex flex-col gap-0.5 py-1">
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="font-mono text-xs tabular-nums text-ink-dim">
                  {formatClock(r.at)}
                </span>
                <span className="font-mono text-[10px] uppercase tracking-wider text-ink-dim">
                  {f.modulation} · {f.encoding}
                </span>
                <span className="font-mono text-xs tabular-nums text-accent">
                  {subghzPayload(f)}
                </span>
                <span className="font-mono text-[10px] tabular-nums text-ink-dim">
                  {subghzTiming(f)}
                </span>
              </div>
              {readings !== "" && (
                <span className="pl-14 font-mono text-[10px] tabular-nums text-ink-dim">
                  {readings}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Digital voice: one call per row, from whichever of the seven modes produced it. There is no
 * audio to offer — none of these modes ships a vocoder here — so the row *is* the decode: who
 * called whom, on which network, and what happened to the transmission. */
export function DvView({ scope = {} }: { scope?: DecoderScope }) {
  const records = recordsInScope(useDecodedKind("dv"), scope);

  if (records.length === 0) {
    return (
      <div className={PANE}>
        <span className={EMPTY}>No digital voice traffic.</span>
      </div>
    );
  }

  return (
    <div className={PANE}>
      <div className="flex flex-col divide-y divide-line">
        {records.map((r, i) => {
          const f = r.event.data;
          const network = dvNetwork(f);
          const parties = dvParties(f);
          const detail = [f.via == null ? null : `via ${f.via}`, f.opcode ?? null, f.text ?? null]
            .filter((part) => part != null)
            .join(" · ");
          return (
            <div key={`${r.at}-${i}`} className="flex flex-col gap-0.5 py-1">
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="font-mono text-xs tabular-nums text-ink-dim">
                  {formatClock(r.at)}
                </span>
                <span className="font-mono text-xs text-accent">{dvMode(f)}</span>
                <span className="font-mono text-xs text-ink">{parties || dvKind(f)}</span>
                {parties !== "" && (
                  <span className="font-mono text-[10px] text-ink-dim">{dvKind(f)}</span>
                )}
                {network !== "" && (
                  <span className="font-mono text-[10px] tabular-nums text-ink-dim">{network}</span>
                )}
                {f.encrypted === true && (
                  <span className="font-mono text-[10px] text-warn">encrypted</span>
                )}
              </div>
              {detail !== "" && (
                <span className="pl-14 font-mono text-[10px] text-ink-dim">{detail}</span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** The view each decoder kind is read in. Keyed on the generated `DecoderKind`, so a decoder
 * added to `wire` fails to compile here until it has somewhere to be read. */
const VIEWS: Record<DecoderKind, (scope: DecoderScope) => ReactNode> = {
  rds: (scope) => <RdsView scope={scope} />,
  adsb: (scope) => <TargetsView kind="adsb" scope={scope} />,
  ais: (scope) => <TargetsView kind="ais" scope={scope} />,
  aprs: (scope) => <AprsView scope={scope} />,
  pocsag: (scope) => <PagerView scope={scope} />,
  rtty: (scope) => <TextView kind="rtty" scope={scope} />,
  morse: (scope) => <TextView kind="morse" scope={scope} />,
  navtex: (scope) => <NavtexView scope={scope} />,
  acars: (scope) => <AcarsView scope={scope} />,
  subghz: (scope) => <SubghzView scope={scope} />,
  dv: (scope) => <DvView scope={scope} />,
};

// `ChannelDescriptor.decoder_kind` is a bare string on the wire, so a server newer than this
// client can name a decoder there is no view for.
function isDecoderKind(kind: string): kind is DecoderKind {
  return Object.hasOwn(VIEWS, kind);
}

/** What a decoder channel's face shows under its settings; nothing for a kind this client has
 * no view for. */
export function DecoderView({ kind, scope }: { kind: string; scope: DecoderScope }) {
  return isDecoderKind(kind) ? VIEWS[kind](scope) : null;
}
