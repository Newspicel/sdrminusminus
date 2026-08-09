# Adding a decoder

The architecture is judged by how cheaply a new decoder can be added. A decoder should touch
**one module in `channels`, one settings struct and one event variant in `wire`, and
optionally one React panel** — nothing else. If your change needs more, stop and reconsider
the design.

Work through it in this order; each step is testable before the next one exists.

## 1. Settings and events in `wire`

Two additions in `crates/wire`.

**Settings** in `channel.rs`: a params struct with `#[serde(default)]` on every field and a
`Default` impl, then a variant in `ChannelParams` and its arm in `type_id()`. The enum is
adjacently tagged, so `{"type":"yourmode","settings":{}}` must deserialize with every field at
its default — a contract test locks that.

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct YourParams {
    #[serde(default = "default_bandwidth_hz")]
    pub bandwidth_hz: f64,
    /// Swap mark and space: some transmitters invert the discriminator polarity.
    #[serde(default)]
    pub invert: bool,
}
```

**Events** in `decode.rs`: a payload struct with the fields the protocol actually carries
(`Option` for anything frame-dependent), a variant in `DecoderEvent`, and arms in `kind()`,
`summary()`, and where applicable `station()` and `position()`. Those four functions are why
the log table, the CSV export, the map and the panels render an event consistently — implement
them and you get all four.

Include the protocol's own interop format in the payload when it has one (`!AIVDM` for AIS,
the TNC2 line for APRS, the raw hex frame for Mode S). It costs one field and makes the log
useful to every other tool.

Add contract tests for the new tags and defaults, then run `cargo xtask codegen` and commit
`openapi.json` and `web/src/generated`.

## 2. Primitives in `dsp`

Look before you write: `crates/dsp` already has FIR design (low-pass, band-pass, Gaussian),
polyphase decimators, a fractional resampler, an FM discriminator, AGC, squelch, PLL and
Costas loops, Gardner symbol sync and a zero-crossing bit clock, NRZI and differential coding,
an HDLC deframer, a G3RUH scrambler, a sync-word correlator, CRC/BCH/Golay/Hamming, Goertzel
and sliding-DFT tone detectors, and an adaptive keying slicer.

Anything genuinely new belongs in `dsp` as a pure, allocation-free primitive with analytic or
golden-vector tests — not buried in your channel module where the next decoder cannot find it.

## 3. The channel module

One file in `crates/channels`, following the shape of the existing ones:

```rust
static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "yourmode".to_owned(),
    name: "Your Mode".to_owned(),
    bandwidth_hz: 12_500.0,
    input_rate_hz: 48_000.0,
    has_audio: false,
    decoder_kind: Some("yourmode".to_owned()),
});
```

Choose `input_rate_hz` as the lowest rate that carries the signal. It is the DDC's output
rate and it dominates the CPU cost: RTTY and Morse run at 8 kHz, not 48 kHz, because a
400 Hz filter at 48 kHz needs thousands of taps to keep its shape factor and blows the
Raspberry Pi 4 budget for a single channel.

Then implement `ChannelRx`:

```rust
pub trait ChannelRx: Send {
    fn descriptor() -> &'static ChannelDescriptor where Self: Sized;
    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> where Self: Sized;
    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError>;
    fn retuned(&mut self) {}
    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs);
}
```

Rules that matter:

- **`process` is the hot path.** No allocation in steady state, no locks, no async, no
  formatting. Push owned `DecoderEvent` values into `out.events`; the host stamps them with
  wall-clock time and absolute frequency off the DSP thread. A decoder never serializes JSON.
- **Validate in `new` and `apply`.** Reject a params variant belonging to another channel type
  with `InvalidSettings`; the engine rebuilds the pipeline on a type change. Verify
  `ctx.input_rate` with `check_input_rate`.
- **Implement `retuned()` if you accrete state.** A retune means a different signal; whatever
  you have collected describes the one you just left. Offset-only changes do not go through
  `apply`, so this hook is the only notification you get.
- **Handle the squelch contract.** A gated span arrives as silence of the same duration, not
  as missing time — your bit clock keeps running, which is the point.
- Register `occupied_band` for your params in `channels::lib.rs`. It is what stops a channel
  whose real occupancy exceeds the passband from silently truncating, and it is computed from
  the configured parameters, not the descriptor nominal.
- Add one row to `REGISTRY`. The descriptor list and the `create` dispatch come from that same
  row, so they cannot drift.

## 4. A reference modulator

In `channels::testgen`, behind the crate's `test-signals` feature. One encoder per protocol,
shared by three consumers: the decoder's unit tests, the engine end-to-end run, and
`cargo xtask fixtures`. The feature is test-only, so `channels` still depends on nothing but
`dsp` and `wire`.

**Write the encoder independently of the decoder.** If both use the same table, a mistyped
constant cancels out and every test passes on a decoder that is wrong. ADS-B's CPR, Gillham
and callsign encoders use closed forms where the decoder uses tables, for exactly this reason,
and it caught a real bug.

## 5. Tests

| Level | Where | Asserts |
|---|---|---|
| Unit | your module | Exact decode from the reference modulator over ragged block boundaries; rejection of noise; every settings knob has an effect |
| End-to-end | `crates/engine/tests/decode.rs` | Reference transmission → SigMF pair → `virtual:file:` playback → DDC **at a different device rate** → your decoder → the engine's decoded broadcast, asserting the message plus the device set, channel and absolute frequency it is stamped with |
| Fixture | `xtask fixtures` + `fixtures/README.md` | A playable SigMF pair with a documented channel, offset and expected output |

Use a device rate that differs from your channel rate in the end-to-end run. Running at the
channel rate bypasses the resampler and hides exactly the class of bug that run exists to
catch.

Assert on decoded *content*, not on "some events arrived". A test whose window is too short to
close a frame passes vacuously over an empty list — that has happened here.

Pure noise must decode to nothing. An idle band that prints garbage is worse than a decoder
that prints nothing.

## 6. The client

Nothing is required: the settings form is generated from the params union (an unhandled
variant fails typecheck), and unknown decoder kinds fall back to the generic log row.

A dedicated panel is worth it when the protocol has a natural view — a target table, a map
layer, a rolling transcript. Route it by `(device_set, channel)`, never by channel id alone:
channel ids are per device set, and keying on the id alone pours two sets' frames into one
panel. Age out or cap anything you index by station, or a long unattended session leaks one
entry per sender.

## 7. Gates

```sh
cargo xtask check
cargo xtask test
```

Then update `PROGRESS.md` with what landed — including the honest gaps, such as whether the
decoder has an off-air capture or only its reference modulator.
