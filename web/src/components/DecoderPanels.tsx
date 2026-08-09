// Per-decoder live views (PLAN §13). All of them read the decoded store, never TanStack Query:
// decoder frames are a stream, not server state. The projection/format/sort logic lives in
// `decoderViews.ts`; these components only render it.
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useDecodedKind, useDecodedStore, useStations } from "../lib/decoded";
import type { DecodedRecordOf } from "../lib/types";
import { BTN, FIELD } from "./controls";
import {
  ageClass,
  aircraftRow,
  aprsMotion,
  buildTranscript,
  type DecoderScope,
  formatAge,
  formatAltFreqs,
  formatClock,
  formatPosition,
  formatRic,
  functionLabel,
  isAtBottom,
  latestWpm,
  matchesAddress,
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
} from "./decoderViews";

const PANE = "flex flex-col gap-2 px-4 py-3";
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

export function TargetsView({ scope = {} }: { scope?: DecoderScope }) {
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

  return (
    <div className={PANE}>
      <TargetTable
        title="Aircraft"
        idHeader="ICAO"
        labelHeader="Callsign"
        primaryHeader="Altitude"
        secondaryHeader="Speed / track"
        rows={sortTargets(
          aircraft.map((s) => aircraftRow(s, now)),
          sort,
          descending,
        )}
        sort={sort}
        descending={descending}
        onSort={toggle}
      />
      <TargetTable
        title="Ships"
        idHeader="MMSI"
        labelHeader="Name"
        primaryHeader="Speed"
        secondaryHeader="Course / destination"
        rows={sortTargets(
          ships.map((s) => shipRow(s, now)),
          sort,
          descending,
        )}
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
