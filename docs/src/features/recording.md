# Recording and replay

sdr-- records raw IQ to [SigMF](https://sigmf.org) and replays a recording as if it were a
device. That symmetry is the whole design: playback needs no new endpoints, no special mode
and no separate UI — a recording probes as a device, and "play" is the ordinary open-a-device
flow.

## Recording

Press record on a device set, or:

```http
POST /api/devicesets/{ds}/record
{"action": "start"}
```

Stop with `{"action": "stop"}`. Files land in `--recordings-dir` (default
`<platform data dir>/sdrmm/recordings`).

Each recording is a SigMF pair:

| File | Contents |
|---|---|
| `<stem>.sigmf-meta` | JSON metadata: `core:datatype` (`cf32_le`), `core:sample_rate`, `core:hw`, and one capture segment per centre frequency |
| `<stem>.sigmf-data` | Interleaved complex float32, little-endian |

The format is fixed to cf32 by decision, not omission — a format field returns when there is a
second format to choose.

While a recording runs, the device set carries a live status: stem, start time, samples,
bytes, capture-ring overruns, and an error if the writer faulted.

### It is lossless, or it fails loudly

The recorder tap sits on the DSP thread and copies each slice into a bounded queue drained by
a dedicated writer thread. If the queue overflows or the writer dies, the tap is **disarmed
and the failure is surfaced** as an error on the device set. A recording never silently loses
samples — a truncated capture that looks complete is worse than no capture.

Capture-ring overruns are a different failure, upstream of the tap: the file stays contiguous
as the DSP plane saw the stream, so a growing `overruns` count means the recording has real
gaps in it. That count is stored with the recording status for exactly that reason.

### Crash safety

Recordings survive an unclean exit in a state that is either "in progress" or "complete",
never "listed but unreadable":

- A `.sigmf-meta.tmp` breadcrumb is written at creation; the real `.sigmf-meta` appears only
  at finalize, via a synced temp rewrite plus an atomic rename.
- The data file and the breadcrumb are claimed with `create_new`, so two concurrent starts
  cannot fight over one stem — the loser retries with a suffix instead of truncating the
  winner's live file.
- The reader tolerates a torn tail, so a hard kill costs you the last block, not the file.
- Live recordings are finalized on device fault, on device-set removal, and on process
  shutdown. Both binaries finalize on their exit path.
- Changing the device sample rate is rejected while recording — one SigMF metadata document
  cannot honestly describe two rates. Centre-frequency retunes *are* allowed and recorded as
  additional capture segments.

## The recordings browser

`GET /api/recordings` lists what is on disk with rate, duration, size, the device it came
from, and the `device_id` that replays it. The index is reconciled from the files themselves
on every listing: the SigMF pairs are the source of truth, the database row is a cache. Delete
removes both.

The panel is visible with no device set open — the library is device-independent.

## Replay

A finalized recording probes as `virtual:file:<stem>`. Open it like any other device:

```http
POST /api/devicesets  {"device_id": "virtual:file:<stem>"}
```

The playback device pins its capabilities to the recording: the centre frequency is fixed
(min equals max) and the sample rate is the single recorded one, so the UI cannot offer
knobs the file cannot honour. It is paced to real time, exposes a `loop` extra setting you
can toggle live, and parks at EOF when looping is off. An I/O error faults the device set
like a broken radio would.

Everything downstream behaves identically to live hardware: channels, decoders, the spectrum
tap — and recording a playback works too.

## Fixtures

`cargo xtask fixtures` renders one playable SigMF pair per wave-1 decoder from the same
reference modulators the decoder tests use, so a fixture can never drift from what the
decoders are tested against. `fixtures/README.md` documents the channel type and offset for
each one, and each is meant to be *played*: open it, add the named channel, watch the decoder
log fill.

Synthesized fixtures are deterministic and therefore never committed — regenerate them.
Recorded off-air captures are the honest gap: as of M4, every decoder is proven against its
specification via a reference modulator, not against the world. Off-air captures land per
decoder as hardware sessions produce them.
