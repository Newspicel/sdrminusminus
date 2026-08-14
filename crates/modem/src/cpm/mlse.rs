//! The sequence-detection tier ( §3.1 `cpm/`, §7 phase-3 follow-on): the detector
//! that reads a *partial-response* entry the way the discriminator tier cannot, by deciding a
//! symbol from the whole span of observations its pulse touches instead of from the one sample
//! its peak lands on.
//!
//! **What the tier is for, measured.** A CPM entry whose frequency pulse spans L symbols puts
//! each symbol's energy across L observation instants, so the eye at any one instant is closed
//! by its neighbours. The discriminator tier answers that with a *filter* choice, and the
//! committed GMSK rows show exactly what that costs: at BT = 0.5 the pulse-matched receive
//! filter is best and the entry reaches 1e-3 at 13.96 dB, but at BT = 0.3 the matched filter's
//! ISI closes the inner eye so badly that the committed chain deliberately runs an *unmatched*
//! BT = 0.5 Gaussian — trading noise bandwidth for an open eye — and still needs 20.95 dB
//! (`CATALOG.md`, and `gmsk_rx`'s six-candidate measurement in
//! `tests/gmsk_msk_afsk_bundles.rs`). This tier removes the trade: the matched filter comes
//! back, and the ISI it re-introduces is what the trellis is *for*.
//!
//! **Measured, against the tier it merges against** (§5 item 2, gated in
//! `the_mlse_tier_beats_the_discriminator_tier_it_merges_against`): at BER 1e-3 the trellis
//! takes GMSK BT = 0.3 from **20.95 dB to 12.80 dB — 8.15 dB** — and BT = 0.5 from 13.96 dB to
//! 12.04 dB, 1.92 dB. The asymmetry is the argument for the tier in one number: BT = 0.5
//! spreads a symbol over 3 symbol-spaced taps and a slicer loses little, BT = 0.3 spreads it
//! over 5 and the eye a slicer needs is not open at all. It costs throughput in proportion to
//! the trellis it runs — 98.8 → 63.8 Msamples/s at 4 states, → 38.0 at 16 — which at 4800 baud
//! is still 791× real time.
//!
//! Two properties the sensitivity number does not carry, both committed in
//! `gmsk_mlse_limits.json`: the tier holds CFO and drift comparably to the slicer, but its
//! **sample-clock tolerance is an order tighter** (1953 ppm against the BT = 0.5 slicer row's
//! 19 922). That is structural rather than incidental — a clock error walks the sampling
//! instant, and a slicer only needs the response's peak to stay put while the trellis is
//! matched to the whole shape of it. An entry running this tier wants a disciplined clock, and
//! now has the number that says how disciplined.
//!
//! **The trellis is over the correlative state, not the phase state.**  §3.1 names
//! "MLSE over the phase trellis"; that is the *coherent* reading, and it needs an absolute
//! carrier phase reference this crate does not have yet — carrier recovery (Costas/FLL/
//! decision-directed/pilot) is phase 4's `linear/` work, and no detector may merge ahead of the
//! machinery that would make it honest (§8). What survives differential detection is the CPM
//! memory itself: the discriminator hands over the *instantaneous frequency*, in which the
//! entry is a plain linear ISI channel
//!
//! ```text
//! y[k] = Σ_i c_i · a[k−i] + noise
//! ```
//!
//! whose taps ([`SymbolResponse`]) are the entry's own frequency pulse convolved with its own
//! receive filter, sampled at symbol spacing. Sequence detection over *that* memory is the part
//! of the coherent gain reachable without a phase reference, and it is the part BT = 0.3 is
//! losing. The phase-state dimension is what the coherent tier adds on top; its catalog row
//! stays `planned:` until phase 4 provides the loops.
//!
//! **Every tap is derived, never declared.** [`SymbolResponse::of`] convolves
//! [`CpmParams::freq_pulse`] with the receive filter the caller hands
//! [`CpmDemod`](super::CpmDemod), scales into the level units the discriminator emits, and
//! samples at the symbol instant — so an entry's trellis follows from the entry's data with
//! nothing to keep in step by hand. Two consequences worth stating because they are
//! load-bearing rather than incidental:
//!
//! - A **Nyquist** cascade (TX-RRC ⊗ RX-RRC, the DMR/C4FM shape) is zero at every non-zero
//!   symbol multiple *by construction*, so its response truncates to one tap, its trellis to
//!   one state, and this detector to the slicer it would have been. The tier is not "better
//!   detection"; it is ISI removal, which an entry needs only if it has ISI — and the ISI-free
//!   entries say so themselves (`a_nyquist_cascade_has_no_isi_to_remove`).
//! - **1REC/MSK** through an integrate-and-dump is the same story: rect ⊗ rect is a triangle
//!   with nulls at ±T. One tap, no gain, and no reason to run this tier on it.
//!
//! **What comes out.** Hard symbol decisions plus per-bit [`SoftBit`]s on the *same* scale
//! [`Mapping::soft_bits`] calibrates to (half a level spacing of margin reads ±0.5, a full
//! spacing saturates at ±1), because a FEC stage below must not be able to tell which detector
//! tier fed it. The recursion is forward–backward min-sum over the trellis: the forward half
//! *is* the Viterbi/MLSE recursion, and the backward half is what turns a sequence detector
//! into a soft-output one for the same order of cost. Hard decisions were measured identical to
//! exhaustive minimum-distance search over every candidate sequence
//! (`decisions_match_exhaustive_maximum_likelihood`) — the property the tier's name claims, and
//! the only check on it that is not a re-run of the implementation.

use super::params::{CpmParams, Mapping};
use crate::soft::SoftBit;

/// Worst-case ISI a discarded tap may contribute, as a fraction of the decision margin (half
/// the mapping's minimum level spacing). The tap count sets the state count exponentially, so
/// the response has to be truncated somewhere, and the honest place is where a tap stops being
/// able to move a decision: a tap `c` contributes at most `|c|·max_level`, so it is kept while
/// that reaches 0.5 % of the margin it would have to cross. Measured on the GMSK rows, the
/// entries this tier exists for: BT = 0.5 keeps 3 taps and BT = 0.3 keeps 5, both conserving
/// Σtaps = 0.999 — an order finer (0.05 %) selects the identical taps, so the responses are
/// complete rather than merely affordable.
///
/// The rule is per-tap over the contiguous run around the cursor, rather than a share of the
/// response's total mass, because a truncated Nyquist design defeats a mass-based rule
/// outright. A span-8 RRC ⊗ RRC cascade is 0.993 at the cursor and ~1e-4 either side of it —
/// mathematically ISI-free, as intended — but carries ±0.006 of truncation ringing eight
/// symbols out. That ringing is 3 % of the total |tap| mass, so a mass-based window grows to 23
/// taps (4^22 states) reaching for shoulders worth 0.6 % of a decision; a contiguous run stops
/// at the 1e-4 neighbours and gives the one tap the cascade actually is.
const RESIDUAL_ISI: f64 = 0.005;

/// Construction cap on the trellis size. Not a performance budget — a design guard: past this
/// the entry's response is too long for plain sequence detection to be the right answer (the
/// literature's route there is a Laurent decomposition onto a shorter effective pulse, which
/// would be its own catalog merge with its own measurement). Panicking with the arithmetic in
/// the message beats a receiver that silently allocates a gigabyte.
const MAX_STATES: usize = 4_096;

/// Symbols per detection window. The window bounds working memory and keeps the steady-state
/// path allocation-free (§4.2); it costs nothing in detection quality because the forward
/// metrics carry across windows unchanged — only the backward half restarts, which is what
/// [`training_tail`] pays for.
const WINDOW_SYMBOLS: usize = 64;

/// Time constant of the scale tracker, in symbols. [`CpmDemod`](super::CpmDemod) normalises its
/// output by a *peak* estimate whose relationship to this model's units depends on the
/// alphabet, the ISI and the tracker's own dynamics (see `cpm::demod`'s `PEAK_SYMBOLS`) — so
/// rather than re-derive that policy here and leave two places to keep in step, the detector
/// measures the scale it is actually handed: the mean |y| of the arriving symbols against the
/// mean its own model predicts.
///
/// Set to the level tracker's own `PEAK_SYMBOLS`, because what this follows *is* that tracker's
/// output and a chaser that settles slower than what it chases lags every transition between
/// two data statistics (a preamble handing over to random payload is exactly that). The number
/// is a principle rather than a measured optimum: 200 symbols was measured indistinguishable on
/// the committed GMSK curves, so nothing here is tuned to a particular framing.
const GAIN_SYMBOLS: f32 = 60.0;

/// Bits a symbol may carry — [`Mapping`] caps M at 256, so eight. A fixed-size soft-bit array
/// is what keeps the per-decision path off the allocator (§4.2).
const MAX_BITS: usize = 8;

/// Backward-recursion training tail, in symbols: how far past a decision the trellis must see
/// before that decision is emitted. Four constraint lengths is the classic Viterbi traceback
/// rule; the floor keeps a one- or two-tap response from restarting the backward pass every few
/// symbols, and the ceiling keeps the emitted fraction of a window worth the work.
fn training_tail(taps: usize) -> usize {
    (4 * taps.saturating_sub(1)).clamp(8, WINDOW_SYMBOLS / 2)
}

/// The entry's end-to-end response at symbol spacing: `taps[t]` is the weight an observation
/// carries for the symbol `t` positions behind the newest one it depends on, in the mapping
/// table's level units. Derived from the entry's own data — never declared — by
/// [`SymbolResponse::of`].
///
/// The main tap (the pulse peak) sits [`lead`](Self::lead) positions in, so a response with
/// anticausal weight — every symmetric pulse pair has some — is carried as plain delay rather
/// than as a special case.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolResponse {
    taps: Vec<f32>,
    lead: usize,
    /// `E|Σ taps·a|` over independent uniform symbols — the model's own mean magnitude, which
    /// the run-time scale tracker measures the arriving symbols against (see [`GAIN_SYMBOLS`]).
    mean_abs: f32,
}

impl SymbolResponse {
    /// The response of `params`'s frequency pulse through `receive_filter`, in the units the
    /// discriminator tier delivers.
    ///
    /// Both filters are unit-area (the [`CpmParams`] and [`CpmDemod`](super::CpmDemod)
    /// contract) and the discriminator is scaled so a sustained symbol at level L reads L —
    /// which is exactly the statement that the symbol-spaced taps of `sps·(g ⊛ r)` sum to one.
    /// The cascade is sampled about the peak of `g ⊛ r`, the instant a timing loop locks to,
    /// with linear interpolation for fractional `sps`. Design math is f64 and cold-path, per
    /// the [`crate::pulse`] convention.
    ///
    /// # Panics
    /// If `receive_filter` is empty, or the cascade has no energy to normalise against.
    #[must_use]
    pub fn of(params: &CpmParams, receive_filter: &[f32]) -> Self {
        assert!(!receive_filter.is_empty(), "receive filter must have taps");
        let cascade = convolve(params.freq_pulse(), receive_filter);
        let sps = params.sps();
        let peak = cascade
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .map_or(0, |(i, _)| i);

        // The contribution of a[k−i] to y[k] is the cascade i symbol periods from its peak;
        // i < 0 is a not-yet-arrived symbol, which the trellis carries as delay.
        let first = -((peak as f64 / sps).floor() as i64);
        let last = ((cascade.len() - 1 - peak) as f64 / sps).floor() as i64;
        let weights: Vec<f64> = (first..=last)
            .map(|i| sps * interpolate(&cascade, peak as f64 + i as f64 * sps))
            .collect();
        let cursor = (-first) as usize;

        let mapping = params.mapping();
        let floor =
            RESIDUAL_ISI * f64::from(mapping.min_spacing() / 2.0) / f64::from(mapping.max_level());
        let (lo, hi) = keep_window(&weights, cursor, floor);
        let taps: Vec<f32> = weights[lo..=hi].iter().map(|&w| w as f32).collect();
        let mean_abs = model_mean_abs(&taps, params.mapping());
        assert!(mean_abs > 0.0, "the pulse cascade has no energy");
        Self {
            taps,
            lead: cursor - lo,
            mean_abs,
        }
    }

    /// `taps[t]` weights the symbol `t` positions behind the newest one the observation depends
    /// on.
    #[must_use]
    pub fn taps(&self) -> &[f32] {
        &self.taps
    }

    /// Index of the main (pulse-peak) tap: how many not-yet-arrived symbols an observation
    /// already carries, and therefore the detector's structural decision delay.
    #[must_use]
    pub fn lead(&self) -> usize {
        self.lead
    }

    /// Whether the entry carries no symbol-spaced ISI at all — a Nyquist cascade, or a
    /// full-response pulse through its own matched filter. Such an entry gains nothing from
    /// this tier, which is a property of the entry rather than a limit of the detector.
    #[must_use]
    pub fn is_isi_free(&self) -> bool {
        self.taps.len() == 1
    }
}

/// Sequence detector over an entry's [`SymbolResponse`]. Streaming: [`process`](Self::process)
/// carries trellis state across calls, so any block split gives the same decisions, and
/// [`flush`](Self::flush) drains the training tail at the end of a transmission.
///
/// Feed it the soft symbols [`CpmDemod`](super::CpmDemod) emits; it replaces
/// [`Mapping::slice`] + [`Mapping::soft_bits`] on the same indexing, so an attachment swaps
/// tiers without touching its framing.
pub struct MlseDetector {
    levels: Vec<f32>,
    m: usize,
    bits_per_symbol: u32,
    /// Metric difference to soft-bit scale. [`Mapping::soft_bits`] reads `0.5 / spacing²`,
    /// which puts a clean symbol's decision at ±0.5 because the two nearest hypotheses are one
    /// level spacing apart *at the one instant it looks at*. The sequence detector compares the
    /// same two hypotheses across every instant the pulse touches, so their separation is
    /// `spacing² · Σ taps²` — the same number for a single-tap (ISI-free) response, and larger
    /// than the slicer's reading of it whenever the pulse spreads energy. Dividing by it is
    /// what makes a clean decision read ±0.5 through either tier, so a FEC stage below cannot
    /// tell which one fed it.
    soft_scale: f32,
    response: SymbolResponse,
    states: usize,
    /// `m^(taps−2)`: the divisor that drops a state's oldest symbol on a transition.
    shift_mask: usize,
    /// Noiseless model output per (state, new symbol) — the trellis as a table rather than a
    /// recomputation (§3.3: what distinguishes entries is data).
    branch: Vec<f32>,
    /// Which transmitted symbol each (state, new symbol) branch *decides*. An observation's
    /// main tap sits [`SymbolResponse::lead`] symbols behind the newest symbol it depends on,
    /// so the branch a step takes carries a not-yet-decidable symbol and the decision owed at
    /// that step is a digit of the state instead. Reading it from a table keeps that offset out
    /// of the hot loop and out of every caller: the output indexes exactly like the slicer's.
    decides: Vec<u8>,
    tail: usize,
    /// Observations awaiting a window, with the scale tracker's value at the instant each
    /// arrived — the backward pass must read the same gain the forward pass did.
    pending: Vec<f32>,
    pending_gain: Vec<f32>,
    mean_abs_y: f32,
    /// Forward metrics, `alpha[k * states + s]`; `alpha[..states]` is what the previous window
    /// carried out.
    alpha: Vec<f32>,
    beta: Vec<f32>,
    beta_next: Vec<f32>,
    /// Combined metric per candidate symbol at one position.
    marginal: Vec<f32>,
    /// Decisions of one window, recorded backwards and emitted in order.
    decisions: Vec<(u8, [f32; MAX_BITS])>,
}

impl MlseDetector {
    /// The detector for an entry and the receive filter its [`CpmDemod`](super::CpmDemod) runs.
    ///
    /// # Panics
    /// If the response's trellis would exceed the construction cap — the message carries M, the tap
    /// count and the resulting state count, because the fix is always one of those three.
    #[must_use]
    pub fn new(params: &CpmParams, receive_filter: &[f32]) -> Self {
        Self::with_response(params, SymbolResponse::of(params, receive_filter))
    }

    /// As [`new`](Self::new) with a response measured elsewhere — the seam the response tests,
    /// and any future Laurent-style approximation, enter through.
    ///
    /// # Panics
    /// As [`new`](Self::new).
    #[must_use]
    pub fn with_response(params: &CpmParams, response: SymbolResponse) -> Self {
        let mapping = params.mapping();
        let m = mapping.m();
        let taps = response.taps.len();
        let states = m
            .checked_pow(taps as u32 - 1)
            .filter(|&s| s <= MAX_STATES)
            .unwrap_or_else(|| {
                panic!(
                    "a trellis of {m}^{} states is past the {MAX_STATES} cap: the response \
                     truncated to {taps} taps",
                    taps - 1
                )
            });
        let mut detector = Self {
            levels: mapping.levels().to_vec(),
            m,
            bits_per_symbol: mapping.bits_per_symbol(),
            soft_scale: 0.5
                / (mapping.min_spacing()
                    * mapping.min_spacing()
                    * response.taps.iter().map(|&c| c * c).sum::<f32>()),
            states,
            shift_mask: m.pow(taps.saturating_sub(2) as u32),
            branch: vec![0.0; states * m],
            decides: vec![0; states * m],
            tail: training_tail(taps),
            pending: Vec::with_capacity(WINDOW_SYMBOLS),
            pending_gain: Vec::with_capacity(WINDOW_SYMBOLS),
            mean_abs_y: response.mean_abs,
            alpha: vec![0.0; (WINDOW_SYMBOLS + 1) * states],
            beta: vec![0.0; states],
            beta_next: vec![0.0; states],
            marginal: vec![0.0; m],
            decisions: Vec::with_capacity(WINDOW_SYMBOLS),
            response,
        };
        detector.fill_tables();
        detector
    }

    /// The entry's response, as derived at construction.
    #[must_use]
    pub fn response(&self) -> &SymbolResponse {
        &self.response
    }

    /// Trellis size — `M^(taps−1)`, and 1 for an ISI-free entry.
    #[must_use]
    pub fn states(&self) -> usize {
        self.states
    }

    /// Detect a block of soft symbols. Appends one decision per input symbol to `symbols`, and
    /// `bits_per_symbol` soft bits per decision to `bits`, MSB first — the order
    /// [`Mapping::soft_bits`] emits and every framing in the workspace reads.
    ///
    /// Output is aligned with the input: index `k` decides the symbol the slicer would have
    /// reported at index `k`. The trailing training-tail symbols stay inside the detector
    /// until more input — or [`flush`](Self::flush) — pushes them out, exactly as a matched
    /// filter holds its last span.
    pub fn process(&mut self, soft: &[f32], symbols: &mut Vec<u8>, bits: &mut Vec<SoftBit>) {
        for &y in soft {
            self.push(y);
            if self.pending.len() == WINDOW_SYMBOLS {
                let emit = WINDOW_SYMBOLS - self.tail;
                self.run_window(WINDOW_SYMBOLS, emit);
                self.emit(symbols, bits);
                self.retain(emit);
            }
        }
    }

    /// Drain the trellis: decide every observation still held, with no future beyond the last.
    /// One transmission's symbols in equals the same number out, once this has been called.
    pub fn flush(&mut self, symbols: &mut Vec<u8>, bits: &mut Vec<SoftBit>) {
        let held = self.pending.len();
        if held == 0 {
            return;
        }
        self.run_window(held, held);
        self.emit(symbols, bits);
        self.retain(held);
    }

    /// Forget the trellis and the scale — the channel moved. There are no filter histories to
    /// wash out; the detector's whole memory is its metrics.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_gain.clear();
        self.decisions.clear();
        self.mean_abs_y = self.response.mean_abs;
        self.alpha.fill(0.0);
    }

    /// The noiseless model output of every (state, new symbol) branch, and the symbol each one
    /// decides. State digit `d` holds the symbol `d + 1` positions behind the branch's own, so
    /// the branch decides its own symbol only for a response with no anticausal weight, and
    /// digit `lead − 1` otherwise.
    fn fill_tables(&mut self) {
        let lead = self.response.lead;
        let digit = self.m.pow(lead.saturating_sub(1) as u32);
        for state in 0..self.states {
            for sym in 0..self.m {
                let mut sum = self.response.taps[0] * self.levels[sym];
                let mut rest = state;
                for &tap in &self.response.taps[1..] {
                    sum += tap * self.levels[rest % self.m];
                    rest /= self.m;
                }
                self.branch[state * self.m + sym] = sum;
                self.decides[state * self.m + sym] = if lead == 0 {
                    sym as u8
                } else {
                    (state / digit % self.m) as u8
                };
            }
        }
    }

    /// Accept one observation, and account it against the scale tracker as it arrives — once
    /// per symbol, whatever windows it later takes part in.
    fn push(&mut self, y: f32) {
        self.mean_abs_y += (y.abs() - self.mean_abs_y) / GAIN_SYMBOLS;
        self.pending.push(y);
        self.pending_gain
            .push(self.mean_abs_y / self.response.mean_abs);
    }

    /// Forward min-sum over `len` observations, then backward min-sum from a flat tail, leaving
    /// the first `emit` positions' decisions in [`Self::decisions`]. The forward half is the
    /// Viterbi recursion; the backward half is what makes the output soft.
    fn run_window(&mut self, len: usize, emit: usize) {
        let Self {
            m,
            states,
            shift_mask,
            branch,
            decides,
            pending,
            pending_gain,
            alpha,
            beta,
            beta_next,
            marginal,
            decisions,
            bits_per_symbol,
            soft_scale,
            ..
        } = self;
        let (m, states, shift_mask) = (*m, *states, *shift_mask);

        for k in 0..len {
            let (done, ahead) = alpha.split_at_mut((k + 1) * states);
            let prev = &done[k * states..];
            let next = &mut ahead[..states];
            next.fill(f32::INFINITY);
            let (y, gain) = (pending[k], pending_gain[k]);
            for (state, &from) in prev.iter().enumerate() {
                for sym in 0..m {
                    let error = y - gain * branch[state * m + sym];
                    let metric = from + error * error;
                    let to = sym + (state % shift_mask) * m;
                    if metric < next[to] {
                        next[to] = metric;
                    }
                }
            }
            // Metrics are differences all the way down; re-basing each step keeps a long
            // transmission's accumulation from spending its mantissa on a common offset.
            let floor = next.iter().copied().fold(f32::INFINITY, f32::min);
            for metric in next.iter_mut() {
                *metric -= floor;
            }
        }

        // A window ends where its observations do, and nothing is known about what follows: a
        // flat backward metric states exactly that, and the training tail is how far its
        // influence is kept from an emitted decision.
        beta_next.fill(0.0);
        decisions.clear();
        for k in (0..len).rev() {
            let (y, gain) = (pending[k], pending_gain[k]);
            beta.fill(f32::INFINITY);
            marginal.fill(f32::INFINITY);
            for state in 0..states {
                let behind = alpha[k * states + state];
                for sym in 0..m {
                    let error = y - gain * branch[state * m + sym];
                    let onward = error * error + beta_next[sym + (state % shift_mask) * m];
                    if onward < beta[state] {
                        beta[state] = onward;
                    }
                    // The marginal is over the symbol this step *decides*, which the response's
                    // lead puts behind the one the branch adds.
                    let decided = decides[state * m + sym] as usize;
                    let total = behind + onward;
                    if total < marginal[decided] {
                        marginal[decided] = total;
                    }
                }
            }
            if k < emit {
                decisions.push(decide(marginal, m, *bits_per_symbol, *soft_scale));
            }
            std::mem::swap(beta, beta_next);
        }
    }

    /// The window's decisions, in transmission order — the backward pass recorded them from the
    /// emit boundary down to the window's start.
    fn emit(&mut self, symbols: &mut Vec<u8>, bits: &mut Vec<SoftBit>) {
        for &(sym, soft) in self.decisions.iter().rev() {
            symbols.push(sym);
            bits.extend(
                soft[..self.bits_per_symbol as usize]
                    .iter()
                    .map(|&b| SoftBit(b)),
            );
        }
    }

    /// Drop the emitted observations, carrying the window's forward metrics at that boundary
    /// into the next window's origin.
    fn retain(&mut self, emitted: usize) {
        let keep = self.pending.len() - emitted;
        self.pending.copy_within(emitted.., 0);
        self.pending.truncate(keep);
        self.pending_gain.copy_within(emitted.., 0);
        self.pending_gain.truncate(keep);
        self.alpha
            .copy_within(emitted * self.states..(emitted + 1) * self.states, 0);
    }
}

/// The decision and per-bit soft values at the position whose combined metrics are in
/// `marginal`: max-log over the candidate symbols, on [`Mapping::soft_bits`]' scale (positive
/// means 1, the crate convention).
fn decide(
    marginal: &[f32],
    m: usize,
    bits_per_symbol: u32,
    soft_scale: f32,
) -> (u8, [f32; MAX_BITS]) {
    let mut best = 0usize;
    for sym in 1..m {
        if marginal[sym] < marginal[best] {
            best = sym;
        }
    }
    let mut soft = [0.0f32; MAX_BITS];
    for k in 0..bits_per_symbol {
        let (mut zero, mut one) = (f32::INFINITY, f32::INFINITY);
        for (sym, &metric) in marginal.iter().enumerate().take(m) {
            if sym >> k & 1 == 0 {
                zero = zero.min(metric);
            } else {
                one = one.min(metric);
            }
        }
        // MSB first, matching Mapping::soft_bits' emission order.
        soft[(bits_per_symbol - 1 - k) as usize] = ((zero - one) * soft_scale).clamp(-1.0, 1.0);
    }
    (best as u8, soft)
}

/// Plain linear convolution in f64. Cold path: once per detector construction.
fn convolve(a: &[f32], b: &[f32]) -> Vec<f64> {
    let mut out = vec![0.0f64; a.len() + b.len() - 1];
    for (i, &x) in a.iter().enumerate() {
        for (j, &h) in b.iter().enumerate() {
            out[i + j] += f64::from(x) * f64::from(h);
        }
    }
    out
}

/// Linear interpolation of `samples` at a fractional index, zero outside. Fractional `sps`
/// (POCSAG's 93.75, AIS's 5) is ordinary in this catalog, so the symbol grid does not in
/// general land on sample instants.
fn interpolate(samples: &[f64], at: f64) -> f64 {
    if at < 0.0 || at >= (samples.len() - 1) as f64 {
        return samples.get(at.round() as usize).copied().unwrap_or(0.0);
    }
    let i = at.floor() as usize;
    let mu = at - i as f64;
    samples[i] * (1.0 - mu) + samples[i + 1] * mu
}

/// The contiguous run of taps around `cursor` that reach `floor` — the span the trellis has to
/// carry. Growth stops at the first tap on each side that cannot move a decision (see
/// [`RESIDUAL_ISI`]); a pulse cascade's energy is contiguous about its peak, so a sub-floor tap
/// is the end of the response and not a hole in it.
fn keep_window(weights: &[f64], cursor: usize, floor: f64) -> (usize, usize) {
    let mut lo = cursor;
    while lo > 0 && weights[lo - 1].abs() >= floor {
        lo -= 1;
    }
    let mut hi = cursor;
    while hi + 1 < weights.len() && weights[hi + 1].abs() >= floor {
        hi += 1;
    }
    (lo, hi)
}

/// `E|Σ taps·a|` over independent uniform symbols — enumerated exactly while the alphabet
/// allows, and estimated from a fixed-seed sample when it does not. Either way a constant of
/// the entry, computed once at construction.
fn model_mean_abs(taps: &[f32], mapping: &Mapping) -> f32 {
    let m = mapping.m();
    let levels = mapping.levels();
    match (m as u64).checked_pow(taps.len() as u32) {
        Some(count) if count <= 1 << 20 => {
            let mut total = 0.0f64;
            for combination in 0..count {
                let mut rest = combination as usize;
                let mut y = 0.0f64;
                for &tap in taps {
                    y += f64::from(tap) * f64::from(levels[rest % m]);
                    rest /= m;
                }
                total += y.abs();
            }
            (total / count as f64) as f32
        }
        _ => {
            const DRAWS: usize = 1 << 16;
            let mut state = 0x9e37_79b9u32;
            let mut total = 0.0f64;
            for _ in 0..DRAWS {
                let mut y = 0.0f64;
                for &tap in taps {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    y += f64::from(tap) * f64::from(levels[state as usize % m]);
                }
                total += y.abs();
            }
            (total / DRAWS as f64) as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::rng::Rng,
        pulse::{self, Norm},
    };

    const SPS: f64 = 10.0;

    fn gmsk(bt: f64, span: usize) -> CpmParams {
        CpmParams::from_h(
            Mapping::natural(2),
            0.5,
            pulse::gaussian_freq(SPS, bt, span, Norm::Area),
            SPS,
        )
    }

    fn gmsk_rx(bt: f64, span: usize) -> Vec<f32> {
        pulse::gaussian_freq(SPS, bt, span, Norm::Area)
    }

    fn symbols(len: usize, seed: u64, m: usize) -> Vec<u8> {
        let mut rng = Rng::new(seed);
        (0..len)
            .map(|_| (rng.next_u64() as usize % m) as u8)
            .collect()
    }

    /// The noiseless observation stream the response model predicts, written out independently
    /// of the branch table so a table bug cannot hide behind itself.
    fn model_stream(response: &SymbolResponse, mapping: &Mapping, sent: &[u8]) -> Vec<f32> {
        let taps = response.taps();
        (0..sent.len())
            .map(|k| {
                let newest = k + response.lead();
                taps.iter()
                    .enumerate()
                    .map(|(t, &tap)| {
                        newest
                            .checked_sub(t)
                            .and_then(|i| sent.get(i))
                            .map_or(0.0, |&s| tap * mapping.level(s))
                    })
                    .sum()
            })
            .collect()
    }

    fn detect(detector: &mut MlseDetector, soft: &[f32]) -> Vec<u8> {
        let mut symbols = Vec::new();
        let mut bits = Vec::new();
        detector.process(soft, &mut symbols, &mut bits);
        detector.flush(&mut symbols, &mut bits);
        symbols
    }

    /// A Nyquist cascade is zero at every non-zero symbol multiple, so the response has exactly
    /// one tap and the trellis one state: the tier reduces to the slicer, and says so.
    #[test]
    fn a_nyquist_cascade_has_no_isi_to_remove() {
        let params = CpmParams::from_deviation(
            Mapping::new(vec![1.0, 3.0, -1.0, -3.0]),
            1_944.0,
            4_800.0,
            pulse::root_raised_cosine(SPS, 0.2, 8, Norm::Area),
            SPS,
        );
        let rx = pulse::root_raised_cosine(SPS, 0.2, 8, Norm::Area);
        let response = SymbolResponse::of(&params, &rx);
        assert!(
            response.is_isi_free(),
            "RRC ⊗ RRC kept {} taps: {:?}",
            response.taps().len(),
            response.taps()
        );
        assert_eq!(MlseDetector::new(&params, &rx).states(), 1);
    }

    /// 1REC through its own integrate-and-dump is a triangle with nulls at ±T — the same story
    /// as Nyquist, and why the MSK row's tier list stops at the discriminator.
    #[test]
    fn a_full_response_pulse_through_its_matched_filter_has_no_isi() {
        let params = CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS);
        let response = SymbolResponse::of(&params, &pulse::rect(SPS, Norm::Area));
        assert!(response.is_isi_free(), "taps {:?}", response.taps());
    }

    /// Partial response is where the tier exists: the Gaussian pulse through its matched filter
    /// spreads a symbol over several instants, and the taps must sum to one — the statement
    /// that a sustained symbol at level L still reads L through a unit-area cascade.
    #[test]
    fn a_gaussian_cascade_spreads_a_symbol_and_conserves_its_level() {
        for (bt, span) in [(0.5, 3), (0.3, 4)] {
            let response = SymbolResponse::of(&gmsk(bt, span), &gmsk_rx(bt, span));
            assert!(
                response.taps().len() >= 3,
                "BT {bt}: only {} taps",
                response.taps().len()
            );
            // Short of exactly one by the truncated tail, which [`RESIDUAL_ISI`] bounds per
            // dropped tap — a level conserved to well inside the decision margin it has to
            // stay clear of.
            let sum: f32 = response.taps().iter().sum();
            assert!((sum - 1.0).abs() < 2e-2, "BT {bt}: Σtaps = {sum}");
            // Symmetric pulses put the peak in the middle; the lead is that peak's index.
            assert_eq!(response.lead(), (response.taps().len() - 1) / 2);
        }
        // The tighter pulse spreads further: BT 0.3 must need at least as many taps as 0.5.
        let wide = SymbolResponse::of(&gmsk(0.3, 4), &gmsk_rx(0.3, 4));
        let narrow = SymbolResponse::of(&gmsk(0.5, 3), &gmsk_rx(0.5, 3));
        assert!(wide.taps().len() >= narrow.taps().len());
    }

    /// The tier's whole claim, on its own model: an ISI pattern that closes the eye at every
    /// instant is still uniquely decodable as a sequence.
    #[test]
    fn noiseless_partial_response_decodes_without_error() {
        let params = gmsk(0.3, 4);
        let rx = gmsk_rx(0.3, 4);
        let response = SymbolResponse::of(&params, &rx);
        let sent = symbols(600, 0x51a7, 2);
        let observed = model_stream(&response, params.mapping(), &sent);
        let mut detector = MlseDetector::new(&params, &rx);
        let got = detect(&mut detector, &observed);
        assert_eq!(got.len(), sent.len(), "one decision per observation");
        // The first `lead` decisions are taken before the trellis has seen a full response, and
        // the last `lead` symbols are only partly observed by a stream that stops with them.
        let span = response.lead()..sent.len() - response.lead();
        let errors = span.clone().filter(|&i| got[i] != sent[i]).count();
        assert_eq!(errors, 0, "sequence errors on a noiseless partial response");
    }

    /// The property the tier's name claims, checked the one way that is not a re-run of the
    /// implementation: against exhaustive minimum-distance search over every candidate
    /// sequence. A short sequence and a two-level alphabet keep the brute force at 2^14.
    #[test]
    fn decisions_match_exhaustive_maximum_likelihood() {
        const LEN: usize = 14;
        let params = gmsk(0.3, 4);
        let rx = gmsk_rx(0.3, 4);
        let response = SymbolResponse::of(&params, &rx);
        let mapping = params.mapping();
        let mut rng = Rng::new(0x3b1e);
        for trial in 0..8u64 {
            let sent = symbols(LEN, 0x100 + trial, 2);
            let clean = model_stream(&response, mapping, &sent);
            // Noise well inside the tier's working range: enough to move decisions, not enough
            // to make the comparison a coin toss.
            let observed: Vec<f32> = clean
                .iter()
                .map(|&y| y + 0.25 * (rng.uniform() as f32 * 2.0 - 1.0))
                .collect();

            let mut best = (f32::INFINITY, Vec::new());
            for code in 0u32..1 << LEN {
                let candidate: Vec<u8> = (0..LEN).map(|i| (code >> i & 1) as u8).collect();
                let distance: f32 = model_stream(&response, mapping, &candidate)
                    .iter()
                    .zip(&observed)
                    .map(|(&p, &y)| (p - y) * (p - y))
                    .sum();
                if distance < best.0 {
                    best = (distance, candidate);
                }
            }

            let mut detector = MlseDetector::new(&params, &rx);
            let got = detect(&mut detector, &observed);
            // The lead-in symbols are never fully observed and the last `lead` are cut off by
            // the end of the block; the interior is what either detector can decide at all.
            let span = response.lead()..LEN - response.lead();
            assert_eq!(
                got[span.clone()],
                best.1[span],
                "trial {trial}: the sequence detector disagreed with exhaustive ML"
            );
        }
    }

    /// A host hands a channel whatever the device gave it; a detector whose decisions depended
    /// on the block split would decode one signal differently on two radios.
    #[test]
    fn block_splits_do_not_change_the_decisions() {
        let params = gmsk(0.3, 4);
        let rx = gmsk_rx(0.3, 4);
        let response = SymbolResponse::of(&params, &rx);
        let observed = model_stream(&response, params.mapping(), &symbols(500, 0x9c4, 2));

        let mut whole = MlseDetector::new(&params, &rx);
        let expected = detect(&mut whole, &observed);

        let mut split = MlseDetector::new(&params, &rx);
        let (mut got, mut bits) = (Vec::new(), Vec::new());
        let mut pos = 0;
        for len in [37usize, 1, 128, 5, 211].iter().cycle() {
            if pos >= observed.len() {
                break;
            }
            let end = (pos + len).min(observed.len());
            split.process(&observed[pos..end], &mut got, &mut bits);
            pos = end;
        }
        split.flush(&mut got, &mut bits);
        assert_eq!(expected, got);
    }

    /// Soft output must carry the scale the slicer tier's does, or a FEC stage below could tell
    /// which detector fed it — and the two tiers would need separate calibrations.
    #[test]
    fn soft_bits_are_signed_and_scaled_like_the_slicer_tier() {
        let params = gmsk(0.5, 3);
        let rx = gmsk_rx(0.5, 3);
        let response = SymbolResponse::of(&params, &rx);
        let observed = model_stream(&response, params.mapping(), &symbols(300, 0x5b17, 2));
        let mut detector = MlseDetector::new(&params, &rx);
        let (mut got, mut bits) = (Vec::new(), Vec::new());
        detector.process(&observed, &mut got, &mut bits);
        detector.flush(&mut got, &mut bits);
        assert_eq!(bits.len(), got.len());
        let bits = &bits[..bits.len() - response.lead()];
        for (i, (&sym, bit)) in got.iter().zip(bits).enumerate().skip(32) {
            assert_eq!(
                bit.bit(),
                sym == 1,
                "bit {i} disagrees with its own hard decision"
            );
            assert!(
                bit.0.abs() <= 1.0,
                "bit {i} soft value {} past scale",
                bit.0
            );
        }
        // A clean stretch must be confident, not merely correct.
        let confident = bits[32..].iter().filter(|b| b.0.abs() > 0.4).count();
        assert!(
            confident > (bits.len() - 32) / 2,
            "only {confident} of {} soft bits carried real confidence",
            bits.len() - 32
        );
    }

    /// The scale tracker exists so the detector reads whatever units the level estimate above
    /// it settled on: a stream scaled by a constant must decode identically.
    #[test]
    fn a_mis_scaled_input_still_decodes() {
        let params = gmsk(0.3, 4);
        let rx = gmsk_rx(0.3, 4);
        let response = SymbolResponse::of(&params, &rx);
        let sent = symbols(800, 0x2ea1, 2);
        let clean = model_stream(&response, params.mapping(), &sent);
        for scale in [0.75f32, 1.0, 1.4] {
            let observed: Vec<f32> = clean.iter().map(|&y| y * scale).collect();
            let mut detector = MlseDetector::new(&params, &rx);
            let got = detect(&mut detector, &observed);
            let errors = (300..sent.len() - response.lead())
                .filter(|&i| got[i] != sent[i])
                .count();
            assert_eq!(errors, 0, "scale {scale}: {errors} errors after settling");
        }
    }

    #[test]
    #[should_panic(expected = "past the")]
    fn an_unreachable_trellis_is_a_construction_error() {
        // Eight levels through a long partial-response pulse: 8^n states past any cap.
        let params = CpmParams::from_h(
            Mapping::natural(8),
            0.25,
            pulse::lrec(SPS, 6, Norm::Area),
            SPS,
        );
        let _ = MlseDetector::new(&params, &pulse::lrec(SPS, 6, Norm::Area));
    }

    /// §4.2: the steady-state detection path may not allocate. Two warm-up calls first, so the
    /// window buffers hold their capacity — the `CpmDemod` gate's convention.
    #[test]
    fn steady_state_detection_allocates_nothing() {
        let params = gmsk(0.3, 4);
        let rx = gmsk_rx(0.3, 4);
        let response = SymbolResponse::of(&params, &rx);
        let observed = model_stream(&response, params.mapping(), &symbols(1_200, 0x0dd5, 2));
        let mut detector = MlseDetector::new(&params, &rx);
        let mut got = Vec::with_capacity(observed.len() * 2);
        let mut bits = Vec::with_capacity(observed.len() * 2);
        detector.process(&observed, &mut got, &mut bits);
        got.clear();
        bits.clear();
        detector.process(&observed, &mut got, &mut bits);
        got.clear();
        bits.clear();
        crate::ber::perf::assert_no_alloc("MlseDetector::process", || {
            detector.process(&observed, &mut got, &mut bits);
        });
        assert!(!got.is_empty(), "the measured call decided nothing");
    }
}
