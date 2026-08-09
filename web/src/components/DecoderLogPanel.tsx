// Decoder log browser (PLAN §11: decoder logs are queryable and exportable, not scroll-back-
// only). The stored page is server state — one TanStack Query keyed by the filter, so changing a
// filter refetches through the key and a `decoder_log` StateChanged invalidates every filter at
// once. The live toggle prepends the WS store's tail on top of that page without refetching.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { clearDecoderLog, DECODER_LOG_KEY, decoderLogExportUrl, decoderLogQuery } from "../lib/api";
import { useDecodedStore } from "../lib/decoded";
import type { DeviceSet } from "../lib/types";
import { BTN, FIELD } from "./controls";
import {
  buildRows,
  collectLive,
  DEFAULT_LOG_FILTER,
  deviceSetOptions,
  droppedNotice,
  formatLogTime,
  isFiltered,
  kindLabel,
  kindOptions,
  LIMIT_OPTIONS,
  type LogFilter,
  toQuery,
} from "./decoderLog";
import { formatMhz } from "./format";

const SEARCH_DEBOUNCE_MS = 250;
const CLEAR_ARM_MS = 3000;
const CELL = "px-2 py-1 align-top";
const HEAD = "px-2 py-1 text-left text-[10px] font-semibold uppercase tracking-wider text-ink-dim";

const NO_FRAMES = {};

export function DecoderLogPanel({ deviceSets = [] }: { deviceSets?: readonly DeviceSet[] }) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<LogFilter>(DEFAULT_LOG_FILTER);
  const [search, setSearch] = useState(DEFAULT_LOG_FILTER.q);
  const [live, setLive] = useState(false);
  const [armed, setArmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cleared, setCleared] = useState<number | null>(null);

  // Typing must not key a new query per keystroke; the committed `filter.q` is what the query
  // key (and every export link) sees.
  useEffect(() => {
    if (search === filter.q) {
      return;
    }
    const timer = window.setTimeout(
      () => setFilter((f) => ({ ...f, q: search })),
      SEARCH_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [search, filter.q]);

  useEffect(() => {
    if (!armed) {
      return;
    }
    const timer = window.setTimeout(() => setArmed(false), CLEAR_ARM_MS);
    return () => window.clearTimeout(timer);
  }, [armed]);

  const query = toQuery(filter);
  const log = useQuery(decoderLogQuery(query));
  // Subscribing to the store only while live is on keeps the panel off the 10 Hz flush entirely
  // when it is showing stored rows.
  const frames = useDecodedStore((s) => (live ? s.frames : NO_FRAMES));
  const lost = useDecodedStore((s) => s.lost);

  const entries = useMemo(() => log.data?.entries ?? [], [log.data]);
  const rows = useMemo(
    () => buildRows(entries, live ? collectLive(frames, filter) : []),
    [entries, frames, filter, live],
  );

  const clearMut = useMutation({
    mutationFn: () => clearDecoderLog(query),
    onSuccess: (deleted) => {
      setError(null);
      setCleared(deleted);
    },
    onError: (e) => setError(e.message),
    onSettled: () => {
      setArmed(false);
      void queryClient.invalidateQueries({ queryKey: DECODER_LOG_KEY });
    },
  });

  const patch = (next: Partial<LogFilter>): void => {
    setCleared(null);
    setFilter((f) => ({ ...f, ...next }));
  };

  const dropped = droppedNotice(live ? lost : 0, log.data?.dropped ?? 0);
  const total = log.data?.total ?? 0;

  return (
    <div className="flex min-h-0 flex-col gap-2 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <select
          className={`${FIELD} text-sm`}
          value={filter.kind}
          onChange={(e) => patch({ kind: e.target.value })}
          aria-label="Decoder"
        >
          <option value="">All decoders</option>
          {kindOptions(entries).map((k) => (
            <option key={k} value={k}>
              {kindLabel(k)}
            </option>
          ))}
        </select>

        <select
          className={`${FIELD} text-sm`}
          value={filter.deviceSet}
          onChange={(e) => patch({ deviceSet: e.target.value })}
          aria-label="Device set"
        >
          <option value="">All devices</option>
          {deviceSetOptions(entries, deviceSets).map((id) => (
            <option key={id} value={String(id)}>
              Set {id}
            </option>
          ))}
        </select>

        <input
          className={`${FIELD} min-w-0 flex-1 text-sm`}
          placeholder="Search station or summary"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Search decoder log"
        />

        <select
          className={`${FIELD} text-sm`}
          value={String(filter.limit)}
          onChange={(e) => patch({ limit: Number(e.target.value) })}
          aria-label="Row limit"
        >
          {LIMIT_OPTIONS.map((n) => (
            <option key={n} value={String(n)}>
              {n} rows
            </option>
          ))}
        </select>

        <button
          type="button"
          className={`${BTN} ${live ? "border-accent text-accent" : ""}`}
          aria-pressed={live}
          onClick={() => setLive(!live)}
        >
          Live
        </button>

        <a className={BTN} href={decoderLogExportUrl("csv", query)} download>
          CSV
        </a>
        <a className={BTN} href={decoderLogExportUrl("json", query)} download>
          JSON
        </a>

        <button
          type="button"
          className={`${BTN} hover:border-danger hover:text-danger ${
            armed ? "border-danger text-danger" : ""
          }`}
          disabled={clearMut.isPending}
          onClick={() => (armed ? clearMut.mutate() : setArmed(true))}
        >
          {armed ? "Confirm clear" : "Clear"}
        </button>
      </div>

      {error !== null && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          <span>Rejected: {error}</span>
          <button type="button" className="shrink-0 underline" onClick={() => setError(null)}>
            dismiss
          </button>
        </div>
      )}

      {log.isError && (
        <div
          role="alert"
          className="rounded border border-danger bg-danger/10 px-3 py-1.5 font-mono text-sm text-danger"
        >
          Log unavailable: {log.error.message}
        </div>
      )}

      {dropped !== null && (
        <div
          role="status"
          className="rounded border border-danger/50 px-3 py-1.5 font-mono text-xs tabular-nums text-danger"
        >
          {dropped}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-x-3 font-mono text-[10px] tabular-nums text-ink-dim">
        <span>
          {rows.length} shown · {total} stored
        </span>
        {armed && (
          <span className="text-danger">Clear removes every row matching this filter.</span>
        )}
        {cleared !== null && <span>{cleared} rows cleared.</span>}
      </div>

      <div className="min-h-0 max-h-80 overflow-auto rounded border border-line">
        <table className="w-full border-collapse font-mono text-xs">
          <thead className="sticky top-0 bg-panel">
            <tr className="border-b border-line">
              <th className={HEAD}>Time UTC</th>
              <th className={HEAD}>Kind</th>
              <th className={`${HEAD} text-right`}>Frequency</th>
              <th className={HEAD}>Station</th>
              <th className={HEAD}>Summary</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr
                key={row.key}
                className={`border-b border-line/50 ${row.live ? "bg-accent/5" : ""}`}
              >
                <td
                  className={`${CELL} whitespace-nowrap tabular-nums text-ink-dim`}
                  title={row.at}
                >
                  <span
                    className={`mr-1.5 inline-block h-1.5 w-1.5 rounded-full align-middle ${
                      row.live ? "bg-accent" : "bg-transparent"
                    }`}
                    aria-label={row.live ? "live" : undefined}
                  />
                  {formatLogTime(row.at)}
                </td>
                <td className={`${CELL} whitespace-nowrap text-ink-dim`}>{kindLabel(row.kind)}</td>
                <td className={`${CELL} whitespace-nowrap text-right tabular-nums text-ink`}>
                  {formatMhz(row.freqHz)}
                </td>
                <td className={`${CELL} whitespace-nowrap text-ink`}>{row.station ?? "—"}</td>
                <td className={`${CELL} max-w-0 truncate text-ink`} title={row.summary}>
                  {row.summary}
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        {rows.length === 0 && (
          <div className="px-3 py-2 text-sm text-ink-dim">
            {log.isPending
              ? "Loading…"
              : isFiltered(filter)
                ? "No rows match this filter."
                : "No decodes logged yet."}
          </div>
        )}
      </div>
    </div>
  );
}
