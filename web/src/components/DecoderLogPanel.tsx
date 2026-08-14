// Decoder log browser (: decoder logs are queryable and exportable, not scroll-back-
// only). The stored page is server state — one TanStack Query keyed by the filter, so changing a
// filter refetches through the key and a `decoder_log` StateChanged invalidates every filter at
// once. The WS store's tail is merged into that page, so a frame is on screen the moment it is
// decoded rather than at the writer's next flush; the log is a live readout, never a page the
// operator has to ask to be current.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, useEffect, useMemo, useState } from "react";
import { clearDecoderLog, DECODER_LOG_KEY, decoderLogExportUrl, decoderLogQuery } from "../lib/api";
import { useDecodedStore } from "../lib/decoded";
import { BTN, FIELD } from "./controls";
import { eventDetail } from "./decoderDetail";
import {
  buildRows,
  collectLive,
  DEFAULT_LOG_FILTER,
  droppedNotice,
  formatLogTime,
  isFiltered,
  kindLabel,
  kindOptions,
  LIMIT_OPTIONS,
  type LogFilter,
  type LogRow,
  sourceSet,
  sourceSets,
  toQuery,
  type WireScope,
} from "./decoderLog";
import { formatMhz } from "./format";
import { Select } from "./Select";

const SEARCH_DEBOUNCE_MS = 250;
const CLEAR_ARM_MS = 3000;
const CELL = "px-2 py-1 align-top";
const HEAD = "px-2 py-1 text-left text-[10px] font-semibold uppercase tracking-wider text-ink-dim";

const NO_FRAMES = {};

/**
 * `wires` is the scope: the decoders feeding this log, by patch node and by the coordinates those
 * nodes hold right now. It narrows both the stored page and the live tail, and it is not a
 * control the operator can clear — a log node shows what is wired into it, and the dropdowns
 * below only narrow further.
 */
export function DecoderLogPanel({ wires }: { wires: WireScope }) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<LogFilter>(DEFAULT_LOG_FILTER);
  const [search, setSearch] = useState(DEFAULT_LOG_FILTER.q);
  const [armed, setArmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cleared, setCleared] = useState<number | null>(null);
  // One row open at a time, keyed by `LogRow.key`: a page of expanded frames is a page nobody
  // can scan, and the key survives the live tail reordering around it.
  const [opened, setOpened] = useState<string | null>(null);

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

  const query = toQuery(filter, wires);
  const log = useQuery(decoderLogQuery(query));
  const frames = useDecodedStore((s) => (wires.sources === "" ? NO_FRAMES : s.frames));
  const lost = useDecodedStore((s) => s.lost);
  const wired = useMemo(() => sourceSet(wires.sources), [wires.sources]);

  const entries = useMemo(() => log.data?.entries ?? [], [log.data]);
  // Straight off the wires, not off the page: a set that is wired in but silent must stay in the
  // list, and a set that is not wired in is a choice that could only ever show nothing.
  const sets = useMemo(() => sourceSets(wires.sources), [wires.sources]);
  const rows = useMemo(
    () => buildRows(entries, collectLive(frames, filter, wired)),
    [entries, frames, filter, wired],
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

  const dropped = droppedNotice(lost, log.data?.dropped ?? 0);
  const total = log.data?.total ?? 0;

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <Select
          label="Decoder"
          value={filter.kind}
          options={[
            { value: "", label: "All decoders" },
            ...kindOptions(entries).map((k) => ({ value: k, label: kindLabel(k) })),
          ]}
          onChange={(kind) => patch({ kind })}
        />

        <Select
          label="Device set"
          value={filter.deviceSet}
          options={[
            { value: "", label: "All devices" },
            ...sets.map((id) => ({ value: String(id), label: `Set ${id}` })),
          ]}
          onChange={(deviceSet) => patch({ deviceSet })}
        />

        <input
          className={`${FIELD} min-w-0 flex-1 text-sm`}
          placeholder="Search station or summary"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Search decoder log"
        />

        <Select
          label="Row limit"
          value={filter.limit}
          options={LIMIT_OPTIONS.map((n) => ({ value: n, label: `${n} rows` }))}
          onChange={(limit) => patch({ limit })}
        />

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
          <span className="text-danger">
            Clear removes every stored row from the decoders wired in that matches this filter.
          </span>
        )}
        {cleared !== null && <span>{cleared} rows cleared.</span>}
      </div>

      {/* Fills the panel it is docked in: a fixed cap would waste a tall panel and overflow a
          short one. */}
      <div className="min-h-0 flex-1 overflow-auto rounded border border-line">
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
              <Fragment key={row.key}>
                {/* The whole row is the control: a summary is truncated by design, and the frame
                    behind it is what the reader is after. */}
                {/* No live/stored distinction is drawn: every row arrives through the tail and
                    turns into its stored twin within a flush, so marking the difference would be
                    half a second of tint on every row in the table. */}
                <tr
                  className="cursor-pointer border-b border-line/50 hover:bg-panel-2"
                  aria-expanded={opened === row.key}
                  onClick={() => setOpened(opened === row.key ? null : row.key)}
                >
                  <td
                    className={`${CELL} whitespace-nowrap tabular-nums text-ink-dim`}
                    title={row.at}
                  >
                    {formatLogTime(row.at)}
                  </td>
                  <td className={`${CELL} whitespace-nowrap text-ink-dim`}>
                    {kindLabel(row.kind)}
                  </td>
                  <td className={`${CELL} whitespace-nowrap text-right tabular-nums text-ink`}>
                    {formatMhz(row.freqHz)}
                  </td>
                  <td className={`${CELL} whitespace-nowrap text-ink`}>{row.station ?? "—"}</td>
                  <td className={`${CELL} max-w-0 truncate text-ink`} title={row.summary}>
                    {row.summary}
                  </td>
                </tr>
                {opened === row.key && (
                  <tr className="border-b border-line/50 bg-panel-2">
                    <td colSpan={5} className="px-3 py-2">
                      <RowDetail row={row} />
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </table>

        {rows.length === 0 && (
          <div className="px-3 py-2 text-sm text-ink-dim">
            {log.isPending
              ? "Loading…"
              : isFiltered(filter)
                ? "No rows match this filter."
                : "Nothing logged yet from the decoders wired in."}
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Everything the frame carried, under the one line the table had room for.
 *
 * This is where a NAVTEX broadcast, an ACARS body and a sub-GHz pulse train are read. The summary
 * column flattens each of them to a line, and before the log became the one place frames are read
 * they had per-channel panes of their own — which were a second copy of this table.
 */
function RowDetail({ row }: { row: LogRow }) {
  const detail = eventDetail(row.event);
  return (
    <div className="flex flex-col gap-2">
      {detail.fields.length > 0 && (
        <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-0.5">
          {detail.fields.map(([label, value]) => (
            <Fragment key={label}>
              <dt className="text-ink-dim">{label}</dt>
              <dd className="min-w-0 break-all text-ink">{value}</dd>
            </Fragment>
          ))}
        </dl>
      )}
      {detail.body !== null && (
        <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded border border-line bg-panel px-2 py-1.5 text-ink">
          {detail.body}
        </pre>
      )}
      {detail.fields.length === 0 && detail.body === null && (
        <span className="text-ink-dim">This frame carried nothing beyond its summary.</span>
      )}
    </div>
  );
}
