# PLAN — native driver migration

Scoped work plan under `PLAN.md` §17 (driver choice) and §3 (crate boundaries). `PLAN.md`
still governs; where this file adds structure it does so inside those rules, and anything
here that contradicts `PLAN.md` must be resolved by editing `PLAN.md` in the same change.

Deliverable is **one PR**, not a push to main — a deliberate exception to `CLAUDE.md`'s
"commit and push directly to main", agreed with the owner for this change.

Private personal project: licence and attribution are explicitly out of scope. There is **no
upstream PR** — this is a fork we own.

---

## 1. Why

Two problems, one cause.

**We depend on two single-maintainer crates the owner does not want as dependencies**, and
each hand-rolls its *own* USB transfer-error policy. Both are wrong, in different ways, and
the divergence is why one of them went unnoticed for a whole milestone.

librtlsdr has had the correct policy for fifteen years (`src/librtlsdr.c:3814`) — cancellations
are excluded from the error path entirely, only genuine errors count, and the threshold is the
queue depth:

```c
} else if (LIBUSB_TRANSFER_CANCELLED != xfer->status) {
    if (LIBUSB_TRANSFER_ERROR == xfer->status ||
            LIBUSB_TRANSFER_TIMED_OUT == xfer->status)
        dev->xfer_errors++;
    if (dev->xfer_errors >= dev->xfer_buf_num ||   /* DEFAULT_BUF_NUMBER 15 */
```

Against that:

| | policy | consequence |
|---|---|---|
| `rs-rtl` 0.4.2 | increments on *every* errored completion, trips at `MAX_CONSECUTIVE_ERRORS = 5` (`rtlsdr.rs:951`, logic `1022-1042`) | one stall = 1 real error + 4 aborted-transfer `Cancelled` = exactly 5 → "dongle disconnected" → full teardown, **~9 s of dead air**. Reproduces from touching the antenna |
| `hackrf-nusb` 0.3.0 | `checked_completion` does `completion.status?`; `accept_completion` then calls `close()`. No counting, no threshold — the crate contains no mention of `Cancelled`, `consecutive`, `retry` or `threshold` | **one** transfer error, `Cancelled` included, kills the stream. Worse than the RTL case. **Newly found — not previously in `PROGRESS.md`** |

We are not switching to C. The `soapy` feature already covers librtlsdr, and making it the
native backend would surrender the single-binary property `cargo xtask dist` gates on (`otool`
check, no libusb).

### Measured facts this plan rests on

Taken on the Nooelec NESDR SMArt v5 (`rtlsdr:89084597`), recorded in `PROGRESS.md`:

- **Restart in place works.** Three consecutive `start_streaming()` calls on one open `RtlSdr`
  each returned in **3 ms**, streaming real samples, with `center_freq`/`sample_rate` intact.
  The full teardown + re-open cost **1622 ms** of bare driver time — before the engine's probe
  and reconnect backoff turn it into the ~9 s the field session saw.
- **rs-rtl's queue is worth keeping.** Under 16 spinning threads it delivered 100.0% of the
  stream at 2.048 and 2.4 MS/s with 0 dropped chunks, where a one-transfer-in-flight driver
  lost ~2.2%.
- **Caveat, unresolved:** that 3 ms was the *clean-stop* path. Neither crate calls nusb's
  `Endpoint::clear_halt`, so restart after a genuinely halted pipe is unproven. The full
  re-open therefore stays as the fallback tier.

---

## 2. Design decisions (locked)

### 2.1 One transport, not two

A new crate `crates/usb-stream` (package `sdrmm-usb-stream`) owns the thing both drivers were
doing differently. **Transport only: raw bytes in, raw bytes out.** No sample conversion, no
register access, no device knowledge.

- Keep `hackrf-nusb`'s `BulkIn` trait seam (`submit` / `wait_next_complete` / `cancel_all`).
  It is the single most valuable thing in either crate: it makes the error policy unit-testable
  against a mock, for *both* drivers, with no hardware (`PLAN.md` §14).
- **Blocking only.** Drop the async half and `maybe_future.rs` — every consumer in this project
  drives the radio from a dedicated capture thread.
- Queue depth 16 in flight (rs-rtl used 15, hackrf-nusb 16), configurable, with rs-rtl's
  blocking handoff and genuine backpressure.
- **Error policy, librtlsdr-shaped:** `Cancelled` never counts; only genuine errors and timeouts
  increment; threshold = queue depth; reset to zero on any successful completion; **resubmit the
  errored transfer and continue**. That last part is rs-rtl's behaviour and librtlsdr does *not*
  do it (it retires errored transfers permanently) — keep ours, it is strictly better.
- **Cancellation needs a stop flag.** Unlike rs-rtl's loop, shutdown here legitimately cancels
  transfers (`close()` → `cancel_all()`). Policy: `Cancelled` while stopping = normal exit;
  `Cancelled` while running = stall fallout → do not count, resubmit.
- Expose received / processed / dropped counters as `hackrf-nusb`'s `StreamingStats` does; both
  backends already surface dropped-chunk warnings.

### 2.2 Every SDR gets a supervisor — policy shared, primitive local

The engine's fault path is one-shot and destructive by design. `RxSink::with_fatal_handler`
takes `on_fatal: impl FnOnce`, which routes to `fault_tx` → the fault drainer →
`mark_device_fault` → `CaptureRuntime::stop`, and `stop` **takes and drops the device** on
purpose (a USB backend holds its interface claim until the handle drops, and auto-reconnect
re-opens the same radio). So a cheap in-place restart cannot live above that seam: once
`RxSink::fail` fires, the device is already going away.

Recovery is therefore **two tiers**:

| tier | where | cost | what it handles |
|---|---|---|---|
| 1 — restart the stream in place | the backend's own capture loop, before `fail` | ~3 ms | transient stalls: the pipe faulted, the device never left the bus |
| 2 — fault → drop → re-probe → re-open | the engine, unchanged | ~9 s | real disconnects, replugs, and any tier-1 failure |

**The policy is device-agnostic and must not be written twice.** It lives as a pure, unit-tested
type in `crates/device` (`sdrmm-device`) — the crate that already defines `SdrDevice` and
`RxSink` and that every backend depends on. It owns attempt counting, backoff schedule and the
give-up decision, and has no I/O.

**The restart primitive is device-specific and stays in each backend**, because reinit differs
(the RTL2832U's `USB_EPA_CTL` FIFO reset lives inside `start_streaming`). The `SdrDevice` trait
does **not** change: each backend's capture loop drives the shared policy and calls its own
restart.

Applicability, stated rather than assumed:

- `device-rtlsdr` — yes. Measured at 3 ms.
- `device-hackrf` — yes. Currently dies on a single errored transfer, so it needs it most.
- `device-soapy` — yes, if a restart primitive exists (`deactivateStream`/`activateStream`).
  Evaluate during the work; if it does not fit cleanly, say so in the PR and leave tier 2.
- `device-virtual` — **no**, and this is not an oversight: it has no transport that can stall.
  Do not force the policy on it.

### 2.3 Conversion stays at the device edge

With a bytes-only core, sample conversion moves out of the transport and into the backends,
where `device-rtlsdr` already keeps it. This is a real change to `device-hackrf`, not a rename:
it currently receives `Complex32` because `hackrf-nusb` converts inline (`convert_cs8`).

`device-hackrf` gains its own pure `convert.rs` on the `device-rtlsdr` pattern. Note the two are
genuinely different and both belong at their own edge: the HackRF is **signed** 8-bit (cs8), the
RTL2832U is **unsigned** with a measured 127.4 DC offset.

---

## 3. Crate layout after the change

**Corrected during the work.** The plan gave each driver its own crate so the fork could be
diffed against upstream. The owner's call is that there is no fork to track — the code is ours
now — so each backend owns its driver as a module instead, and there is one crate per backend:

```
crates/usb-stream            NEW  sdrmm-usb-stream  queue + error policy + BulkIn seam
crates/device                     sdrmm-device      + shared restart policy (pure)
crates/device-rtlsdr/src/driver/  NEW               RTL2832U + R82xx, on usb-stream
crates/device-hackrf/src/driver/  NEW               HackRF radio, on usb-stream
crates/device-*/src/{caps,convert}.rs               pure; lib.rs is thin I/O + supervisor
```

`rs-rtl` and `hackrf-nusb` leave the workspace manifest and `Cargo.lock`. Where the code started
is recorded once, in `PLAN.md` §18, because it explains why "always newest versions" does not
apply to it — not in the crates, which are no longer a fork of anything.

We keep `hackrf-nusb`'s code rather than switching to `rs-hackrf` from the desperado repo: at
~736 lines it is a sixth the size, and our field session (20 Msps zero overruns, per-stage
LNA/VGA gains, 2–20 Msps continuous) is proven against *this* one.

Vendored code now meets this repo's bar — `cargo fmt`, `clippy -D warnings`, workspace lints,
`CLAUDE.md`'s no-useless-comments rule. Fix what the gate flags; do not blanket-allow.

---

## 4. Phases

Ordered so the tree stays green and every phase is revertable on its own. If a later phase turns
hairy, everything before it still lands.

### Phase 1 — `usb-stream`
Build the core with its mock-based tests before either driver depends on it. Port
`hackrf-nusb`'s existing streaming tests (queue-of-16, error-closes-the-queue) onto it; they are
the main safety net for the whole change.

### Phase 2 — RTL driver
Bring `rs-rtl` 0.4.2 in (2711 lines: `device.rs`, `error.rs`, `lib.rs`, `rtlsdr.rs`, `tuner.rs`)
rebuilt on the core. Keep `tuner.rs` (R820T register programming) and `device.rs` (register/I2C)
— that is the valuable, tedious part and it works.

**Drop `StreamControl` and the in-thread retune machinery.** `device-rtlsdr` already bypasses it
deliberately (see the `RtlSdrDevice` doc comment: setters ride the control endpoint because the
queued commands are fire-and-forget, so a rejected retune would look applied). Only `stop()` is
used, and the core replaces it. Do not port dead paths.

`device-rtlsdr` should change only its dependency and `rs_rtl::` → `sdrmm_rtl_driver::`.

### Phase 3 — HackRF driver
Bring `hackrf-nusb` 0.3.0 in (4754 lines src) on the same core, and give `device-hackrf` its
pure `convert.rs` per §2.3. Bigger than a rename — budget for it.

### Phase 4 — supervisor
Shared policy in `sdrmm-device` per §2.2, driven from each applicable backend's capture loop.

**Landed differently, and better:** the *policy* is shared as planned, but so is the loop it
drives. Both backends' capture loops turned out to be the same ~105 lines bar the restart
primitive and the block size, so `sdrmm-device` owns the whole supervisor (`Capture`,
`CaptureRadio`, `CaptureStream`, `SampleConverter`) and a backend supplies only `arm`/`disarm`
and its sample table. Two lifecycle bugs fell out of the split rather than being found by
inspection — see `PLAN.md` §18, "Shared device machinery". Half-duplex arbitration moved up with
it: `Duplex`/`DuplexState` in `sdrmm-device` replaced the HackRF driver's private `claim`/
`select`, and the abstraction now carries the TX half (§12a), which is what made the HackRF's
restored transmit path reachable through `SdrDevice` instead of only through its concrete type.

Constraints:
- **Tier 2 stays the fallback.** A failed restart must fault the set exactly as today. Strictly
  better than current behaviour, never worse. `clear_halt` on the restart path is now a
  legitimate option since we own the code — if added, say so and test it.
- The device must be reachable from the capture thread (`Arc<Mutex<…>>`); **never hold the lock
  across the blocking read** — `apply`'s setters take the same lock.
- **Reset the converter on restart.** `device-rtlsdr`'s `convert.rs` carries a half-sample
  `carry: Option<u8>` across blocks by design, and `capture_loop` builds the converter once
  outside the loop. A stalled transfer can complete short with an *odd* `actual_len`, so a stale
  carry byte prepended to the new stream would swap I and Q for the rest of the session. Add
  `IqConverter::reset()`; check whether the new HackRF converter needs the same guard.
- A restart loses samples — it is not free and must not be silent. `tracing::warn!` with an
  attempt count at minimum. Decide whether it belongs on the wire (see how the engine reports
  faults and reconnects) and justify the choice.

### Phase 5 — PPM correction
Now that we own the driver, do it *in* the driver rather than as the caller-side approximation
`PROGRESS.md` currently describes: librtlsdr-shaped `set_freq_correction(ppm)` correcting **both**
the resampler (demod page 1, regs 0x3f/0x3e) and the tuner crystal, then re-tuning. `rtlsdr-pure`
0.2.3's `set_frequency_correction` / `rtl2832.rs` is a clean reference for the resampler half.

Expose `ppm` on the wire — **corrected during the work: no wire change is needed.**
`DeviceSettings.ppm` has been a first-class field since M1 (it is how `device-soapy` drives the
`"CORR"` component) and the web UI already renders the control, so an extra setting beside
`biastee`/`agc` would have been a second way to say the same thing. Validate in `caps.rs`, apply
in `lib.rs`, stop rejecting it. `settings()` must keep reporting what the hardware holds.

---

## 5. Testing (`PLAN.md` §14: no hardware in CI, ever)

- **Core, against a mock `BulkIn`:** a `Cancelled` burst while running does not trip the
  threshold and the transfers are resubmitted; a genuine-error burst does trip it; a success
  resets the counter; `Cancelled` while stopping is a clean exit, not an error.
- **Restart policy:** pure unit tests on attempts, backoff and the give-up decision.
- **`IqConverter::reset()`:** a stale odd carry cannot leak across a restart.
- **HackRF cs8 → cf32 converter:** golden values, alignment across block boundaries.
- **PPM:** scaling and rounding in `caps.rs`.
- No test may call `probe`/`open` or anything on a device type.

## 6. Hardware verification

An RTL-SDR is attached to the development machine. **A HackRF may not be — check first.**

`cargo xtask dist && ./dist/sdrmm --doctor`, then stream. Both backends must enumerate, tune,
gain, bias-tee and decode exactly as before: apart from the error-policy fixes this is
behaviour-neutral. Prove the restart path directly (force the branch, or drive it from a scratch
binary outside the workspace — never `cargo add` into it). Verify PPM against a known carrier: a
deliberate ±50 ppm should move a local broadcast peak by the predicted amount.

Cheap regression check on the new core: rs-rtl delivered 100.0% of the stream under a 16-thread
load at 2.048 and 2.4 MS/s. The ported path should still do that.

Two things the PR must state plainly rather than imply:
- If no HackRF is attached, the HackRF half is verified **only against mocks** and is the owner's
  first thing to test.
- The one real regression test for the RTL stall — streaming while physically touching the
  antenna — is the owner's to run. No harness reproduces it.

## 7. Definition of done

- `cargo xtask check` and `cargo xtask test` green, including the Soapy-free
  `--no-default-features --features rtl-native,hackrf-native` build.
- `PLAN.md` updated in the same change: §17's native-backend rows go from external dependencies
  to vendored-at-version-plus-sha on a shared transport, with the two error policies as the
  reason; §3 gains `sdrmm-usb-stream`. The "always newest versions" rule is satisfied by
  recording the fork points as deliberate.
- `PROGRESS.md` updated: the RTL transient-stall gap becomes a shipped fix; the HackRF
  single-error-kills-the-stream bug is recorded as newly found and fixed; PPM closes.
- Branch + `gh pr create` carrying the evidence above. **Do not push to main.**

## 8. Out of scope

- **Direct sampling** (HF). Unblocked by the vendoring — it means bypassing the tuner, which was
  impossible while `rs-rtl` held its `R82xx` privately — but not built here. Record it in
  `PROGRESS.md` as the next gap.
- **`device-virtual`** gets no supervisor (§2.2) — it shares the thread plumbing (`Worker`) and
  nothing else, having no transport that can stall.
- **Upstream contributions.** None.
