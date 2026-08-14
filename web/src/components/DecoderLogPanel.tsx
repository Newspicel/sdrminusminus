// Decoder log browser (: decoder logs are queryable and exportable, not scroll-back-
// only). The stored page is server state — one TanStack Query keyed by the filter, so changing a
// filter refetches through the key and a `decoder_log` StateChanged invalidates every filter at
// once. The WS store's tail is merged into that page, so a frame is on screen the moment it is
// decoded rather than at the writer's next flush; the log is a live readout, never a page the
// operator has to ask to be current.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Fragment, useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { clearDecoderLog, DECODER_LOG_KEY, decoderLogExportUrl, decoderLogQuery } from "../lib/api";
import { useDecodedStore } from "../lib/decoded";
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
  matchesFilter,
  sourceSet,
  sourceSets,
  toQuery,
  type WireScope,
} from "./decoderLog";
import { formatMhz } from "./format";
import { InlineAlert } from "./InlineAlert";
import { Select } from "./Select";

const SEARCH_DEBOUNCE_MS = 250;
const CLEAR_ARM_MS = 3000;
const CELL = "px-2 py-1 align-top";
const HEAD =
  "px-2 py-1 text-left text-[10px] font-semibold uppercase tracking-wider text-muted-foreground";

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
      // The tail is this browser's copy of rows the server has just deleted; leaving it renders
      // them for another `RING_CAPACITY` frames, which reads as a Clear that did nothing.
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

        <Input
          className="min-w-0 flex-1"
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

        <Button
          render={<a href={decoderLogExportUrl("csv", query)} download />}
          variant="outline"
          size="sm"
        >
          CSV
        </Button>
        <Button
          render={<a href={decoderLogExportUrl("json", query)} download />}
          variant="outline"
          size="sm"
        >
          JSON
        </Button>

        <Button
          type="button"
          variant="destructive"
          size="sm"
          className={` ${armed ? "border-destructive text-destructive" : ""}`}
          disabled={clearMut.isPending}
          onClick={() => (armed ? clearMut.mutate() : setArmed(true))}
        >
          {armed ? "Confirm clear" : "Clear"}
        </Button>
      </div>

      {error !== null && (
        <InlineAlert className="flex-row items-center justify-between font-mono text-sm">
          <span>Rejected: {error}</span>
          <Button type="button" className="shrink-0 underline" onClick={() => setError(null)}>
            dismiss
          </Button>
        </InlineAlert>
      )}

      {log.isError && (
        <InlineAlert className="font-mono text-sm">
          Log unavailable: {log.error.message}
        </InlineAlert>
      )}

      {dropped !== null && (
        <div
          role="status"
          className="rounded border border-destructive/50 px-3 py-1.5 font-mono text-xs tabular-nums text-destructive"
        >
          {dropped}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-x-3 font-mono text-[10px] tabular-nums text-muted-foreground">
        <span>
          {rows.length} shown · {total} stored
        </span>
        {armed && (
          <span className="text-destructive">
            Clear removes every stored row from the decoders wired in that matches this filter.
          </span>
        )}
        {cleared !== null && <span>{cleared} rows cleared.</span>}
      </div>

      {/* Fills the panel it is docked in: a fixed cap would waste a tall panel and overflow a
          short one. */}
      <div className="min-h-0 flex-1 overflow-auto rounded border border-border">
        <Table className="font-mono text-xs">
          <TableHeader className="sticky top-0 bg-card">
            <TableRow>
              <TableHead className={HEAD}>Time UTC</TableHead>
              <TableHead className={HEAD}>Kind</TableHead>
              <TableHead className={`${HEAD} text-right`}>Frequency</TableHead>
              <TableHead className={HEAD}>Station</TableHead>
              <TableHead className={HEAD}>Summary</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row) => (
              <Fragment key={row.key}>
                {/* The whole row is the control: a summary is truncated by design, and the frame
                    behind it is what the reader is after. */}
                {/* No live/stored distinction is drawn: every row arrives through the tail and
                    turns into its stored twin within a flush, so marking the difference would be
                    half a second of tint on every row in the table. */}
                <TableRow
                  className="cursor-pointer border-b border-border/50 hover:bg-muted"
                  aria-expanded={opened === row.key}
                  onClick={() => setOpened(opened === row.key ? null : row.key)}
                >
                  <TableCell
                    className={`${CELL} whitespace-nowrap tabular-nums text-muted-foreground`}
                    title={row.at}
                  >
                    {formatLogTime(row.at)}
                  </TableCell>
                  <TableCell className={`${CELL} whitespace-nowrap text-muted-foreground`}>
                    {kindLabel(row.kind)}
                  </TableCell>
                  <TableCell
                    className={`${CELL} whitespace-nowrap text-right tabular-nums text-foreground`}
                  >
                    {formatMhz(row.freqHz)}
                  </TableCell>
                  <TableCell className={`${CELL} whitespace-nowrap text-foreground`}>
                    {row.station ?? "—"}
                  </TableCell>
                  <TableCell
                    className={`${CELL} max-w-0 truncate text-foreground`}
                    title={row.summary}
                  >
                    {row.summary}
                  </TableCell>
                </TableRow>
                {opened === row.key && (
                  <TableRow className="bg-muted">
                    <TableCell colSpan={5} className="px-3 py-2">
                      <RowDetail row={row} />
                    </TableCell>
                  </TableRow>
                )}
              </Fragment>
            ))}
          </TableBody>
        </Table>

        {rows.length === 0 && (
          <div className="px-3 py-2 text-sm text-muted-foreground">
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
              <dt className="text-muted-foreground">{label}</dt>
              <dd className="min-w-0 break-all text-foreground">{value}</dd>
            </Fragment>
          ))}
        </dl>
      )}
      {detail.body !== null && (
        <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded border border-border bg-card px-2 py-1.5 text-foreground">
          {detail.body}
        </pre>
      )}
      {detail.fields.length === 0 && detail.body === null && (
        <span className="text-muted-foreground">
          This frame carried nothing beyond its summary.
        </span>
      )}
    </div>
  );
}
