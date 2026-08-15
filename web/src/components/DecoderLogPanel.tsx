import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, useEffect, useMemo, useState } from "react";
import { FaceBody, FaceFooter } from "../canvas/nodes/NodeShell";
import { clearDecoderLog, DECODER_LOG_KEY, decoderLogExportUrl, decoderLogQuery } from "../lib/api";
import { useDecodedStore } from "../lib/decoded";
import { Button, Input } from "./BaseControls";
import { ALERT, BTN, FIELD, TABLE_CELL, TABLE_HEAD } from "./controls";
import { eventDetail } from "./decoderDetail";
import {
  buildRows,
  collectLive,
  DEFAULT_LOG_FILTER,
  droppedNotice,
  isFiltered,
  kindLabel,
  LIMIT_OPTIONS,
  type LogFilter,
  type LogRow,
  matchesFilter,
  sourceSet,
  toQuery,
  type WireScope,
} from "./decoderLog";
import { formatClock } from "./decoderViews";
import { formatMhz } from "./format";
import { Select } from "./Select";

const SEARCH_DEBOUNCE_MS = 250;
const CLEAR_ARM_MS = 3000;

const NO_FRAMES = {};

export function DecoderLogPanel({ wires }: { wires: WireScope }) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<LogFilter>(DEFAULT_LOG_FILTER);
  const [search, setSearch] = useState(DEFAULT_LOG_FILTER.q);
  const [armed, setArmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cleared, setCleared] = useState<number | null>(null);
  const [opened, setOpened] = useState<string | null>(null);

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
  const rows = useMemo(
    () => buildRows(entries, collectLive(frames, filter, wired)),
    [entries, frames, filter, wired],
  );

  const clearMut = useMutation({
    mutationFn: () => clearDecoderLog(query),
    onSuccess: (deleted) => {
      const live = rows.filter((row) => row.live).length;
      useDecodedStore.getState().dropFrames((record) => matchesFilter(record, filter, wired));
      setError(null);
      setCleared(deleted + live);
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
    <>
      <FaceBody scroll={false}>
        <div className="flex min-h-0 flex-1 flex-col gap-2 p-2">
          <div className="flex items-center gap-2">
            <Input
              className={`${FIELD} min-w-0 flex-1`}
              placeholder="Search station or summary"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              aria-label="Search decoder log"
            />
            <Select
              label="Row limit"
              className="w-28 shrink-0"
              value={filter.limit}
              options={LIMIT_OPTIONS.map((n) => ({ value: n, label: `${n} rows` }))}
              onChange={(limit) => patch({ limit })}
            />
          </div>

          {error !== null && (
            <div role="alert" className={`${ALERT} flex items-center justify-between gap-3`}>
              <span>Rejected: {error}</span>
              <Button type="button" className="shrink-0 underline" onClick={() => setError(null)}>
                dismiss
              </Button>
            </div>
          )}

          {log.isError && (
            <div role="alert" className={ALERT}>
              Log unavailable: {log.error.message}
            </div>
          )}

          {dropped !== null && (
            <div role="status" className={`${ALERT} bg-transparent tabular-nums`}>
              {dropped}
            </div>
          )}

          <div className="flex flex-wrap items-center gap-x-3 font-mono text-[10px] tabular-nums text-ink-dim">
            <span>
              {rows.length} shown · {total} stored
            </span>
            {armed && (
              <span className="text-danger">Clear removes every stored row this node can see.</span>
            )}
            {cleared !== null && <span>{cleared} rows cleared.</span>}
          </div>

          <div className="min-h-0 flex-1 overflow-auto rounded border border-line">
            <table className="w-full border-collapse font-mono text-xs">
              <thead className="sticky top-0 bg-panel">
                <tr className="border-b border-line">
                  <th className={TABLE_HEAD}>Time</th>
                  <th className={TABLE_HEAD}>Kind</th>
                  <th className={`${TABLE_HEAD} text-right`}>Frequency</th>
                  <th className={TABLE_HEAD}>Station</th>
                  <th className={TABLE_HEAD}>Summary</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <Fragment key={row.key}>
                    <tr
                      className="cursor-pointer border-b border-line/50 hover:bg-panel-2 focus-visible:outline focus-visible:outline-accent"
                      aria-expanded={opened === row.key}
                      tabIndex={0}
                      onClick={() => setOpened(opened === row.key ? null : row.key)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          setOpened(opened === row.key ? null : row.key);
                        }
                      }}
                    >
                      <td
                        className={`${TABLE_CELL} whitespace-nowrap tabular-nums text-ink-dim`}
                        title={row.at}
                      >
                        {formatClock(row.at)}
                      </td>
                      <td className={`${TABLE_CELL} whitespace-nowrap text-ink-dim`}>
                        {kindLabel(row.kind)}
                      </td>
                      <td
                        className={`${TABLE_CELL} whitespace-nowrap text-right tabular-nums text-ink`}
                      >
                        {formatMhz(row.freqHz)}
                      </td>
                      <td className={`${TABLE_CELL} whitespace-nowrap text-ink`}>
                        {row.station ?? "—"}
                      </td>
                      <td className={`${TABLE_CELL} max-w-0 truncate text-ink`} title={row.summary}>
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
      </FaceBody>
      <FaceFooter>
        <a className={BTN} href={decoderLogExportUrl("csv", query)} download>
          CSV
        </a>
        <a className={BTN} href={decoderLogExportUrl("json", query)} download>
          JSON
        </a>
        <Button
          type="button"
          className={`${BTN} hover:border-danger hover:text-danger ${
            armed ? "border-danger text-danger" : ""
          }`}
          disabled={clearMut.isPending}
          onClick={() => (armed ? clearMut.mutate() : setArmed(true))}
        >
          {armed ? "Confirm clear" : "Clear"}
        </Button>
      </FaceFooter>
    </>
  );
}

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
