# Decoder log and export

Most SDR UIs give you a scroll-back buffer. sdr-- stores decoder output in SQLite, filters it
server-side, and exports it — so "which aircraft did I see last Tuesday" is a query, not a
memory.

## What is stored

One row per decoded event: timestamp, device set, channel, decoder kind, absolute frequency,
station identifier (ICAO address, MMSI, callsign, RIC, PI code — whatever identifies the
sender for that decoder), a one-line summary, and the **verbatim typed event** as JSON. The
summary and station are computed from one shared implementation, so the log table, the CSV
export and the map all describe an event the same way.

The writer batches records into one transaction per batch off the engine's decoded broadcast,
with a retry queue, and prunes periodically to a bounded row count (one million rows — a few
hundred megabytes at roughly 300 B a row, and about three hours of history at a busy ADS-B
site). An unattended receiver must not be able to fill the disk.

Lag and queue overflow are counted and reported as `dropped` on every listing.

## Querying

```http
GET /api/decoderlog?kind=adsb&device_set=0&since=2026-08-01T00:00:00Z&q=DLH&limit=500
```

| Parameter | Effect |
|---|---|
| `kind` | One decoder (`rds`, `pocsag`, `adsb`, `ais`, `aprs`, `rtty`, `morse`) |
| `device_set` | One device set |
| `since` / `until` | RFC3339 time window |
| `q` | Case-insensitive substring match against station and summary |
| `limit` | Page size, server-clamped |

Filters compose, and the response reports the total alongside the returned page.

In the UI the filters live in the query key, so changing one refetches through the normal
cache path — no polling, no manual refresh button.

## Live rows versus stored rows

The two are deliberately separate:

- **Live decodes** arrive as `Decoded` WebSocket events and are appended to a client-side ring
  buffer, batched at 100 ms so an ADS-B burst cannot re-render the page at frame rate.
- **The stored log** is refetched only when it changes *structurally* — cleared or pruned,
  signalled by a `StateChanged { DecoderLog }` event. Invalidating on every decode would
  refetch the whole log hundreds of times a second.

Live rows are visually distinct from stored ones in the **Decoder log** node's face.

## Export

```http
GET /api/decoderlog/export/csv?kind=ais&since=2026-08-01T00:00:00Z
GET /api/decoderlog/export/json?kind=ais&since=2026-08-01T00:00:00Z
```

The same filter as the list endpoint (`limit` is ignored — an export is the whole match),
served as a real download with a timestamped filename. The format is a path segment rather
than a query field because the filter struct is shared by all three endpoints and cannot be
flattened into a query alongside an enum.

CSV columns, RFC 4180 quoted:

```
at,device_set,channel,kind,freq_hz,station,summary,event
```

The last column is the full JSON event, so an export loses nothing the log stored. Load it
into a spreadsheet for the projected columns, or parse `event` for the typed fields.

## Clearing

`DELETE /api/decoderlog` takes the same filter and returns how many rows it removed, then
emits the decoder-log scope so every client refetches. The UI guards the button — a filtered
clear is easy to fire twice.
