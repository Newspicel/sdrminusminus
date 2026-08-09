# Frequency scanner

The scanner is app-level, not a channel type. A control-plane thread per device set walks a
list of target frequencies, measures each one against the **existing** spectrum tap, and
parks a hosted channel on whatever breaks the threshold.

## Why it is cheap

The unit of work is a *device tuning*, not a target. One tuning's passband usually covers
hundreds of targets, and they are all measured from the same spectrum frames — a 2 MHz-wide
receiver sweeps an entire VHF band per dwell instead of one channel at a time.

A scan therefore costs retunes plus a max over FFT bins, and no extra DSP at all. That is what
keeps it affordable on the Raspberry Pi 4, which is the project's performance floor.

Targets are only placed within the usable 80% of the sample rate: the band edges belong to the
device's analog roll-off and the capture filter's transition, and measuring there reads low
and misses signals.

## Configuring a scan

A scan is defined by ranges, individual frequencies, or both. Ranges expand to
`start_hz, start_hz + step_hz, …` up to and including `stop_hz` when it lands on a step. The
expanded list is sorted, deduplicated and bounded at 20 000 targets, so a mistyped `step_hz`
is rejected rather than turning a range into gigabytes of target list.

| Setting | Default | Meaning |
|---|---|---|
| `ranges` | — | Spans to sweep (`start_hz`, `stop_hz`, `step_hz`) |
| `frequencies` | — | Individual targets: bookmarks, memory channels |
| `threshold_db` | −55 dBFS | Level at which a target counts as active, on the device's spectrum tap |
| `dwell_ms` | 250 | Measurement window per **tuning** (not per target) |
| `resume_ms` | 1500 | How long a held target must stay below the threshold before the sweep resumes |
| `measure_bw_hz` | 12 500 | Bandwidth measured around each target, and the width a hold is judged over |
| `hold_channel` | none | Channel retuned onto a hit, so its audio or decoder follows the scan |

A dwell shorter than one spectrum frame would measure nothing, so it is floored at 40 ms.
After each retune the scanner waits briefly before believing a frame — the capture ring still
holds samples from the previous frequency, and measuring those would report the old tuning's
energy at the new one.

Without a `hold_channel` the scan listens to nothing and only logs hits. With one, the channel
is retuned onto each hit, so a scanning receiver behaves like the handheld kind: audio (or a
decoder) follows the activity.

## Running one

```http
POST /api/devicesets/{ds}/scanner
{"action": "start", "settings": { ... }}

POST /api/devicesets/{ds}/scanner
{"action": "stop"}
```

While a scan runs the device set's `scanner` field carries the live status: state
(`scanning` or `holding`), the expanded target count, the current frequency and its measured
level, completed sweeps, total hits, and an error if the scan faulted.

Progress arrives as a dedicated `ScannerUpdate` WebSocket event, rate-limited to a few per
second with state transitions exempt. It is not a `StateChanged`: a scan retunes the device
every dwell, and one full-state refetch per step would cost far more than the scan itself. A
`StateChanged { DeviceSet }` still fires when a scan starts and stops.

## While a scan runs

The device's centre frequency is under the scanner's control and moves constantly. Retuning
it yourself fights the scan; stop the scan first. A fatal scanner fault (the device stops
accepting retunes) stops the scan and leaves the cause visible in the status rather than
quietly ending it.
