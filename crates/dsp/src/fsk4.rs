//! Four-level FSK symbol recovery (PLAN §13 wave 3) — the front end every digital-voice mode
//! but D-Star shares.
//!
//! Chain: carrier gate → FM discriminator → root-raised-cosine matched filter → Gardner symbol
//! clock → level normalisation. What comes out is one soft symbol per symbol period, scaled so
//! the four transmitted levels sit at ±1 and ±3 whatever the transmitter's actual deviation is.
//!
//! The clock is a Gardner detector rather than the zero-crossing tracker the two-level decoders
//! use, because with four levels a crossing is no longer a timing reference: a +1 → −3
//! transition crosses zero a quarter of a symbol before a +3 → −3 one does, and a tracker that
//! believed both would jitter with the data. Gardner's estimate is level-independent.
//!
//! Nothing here knows the deviation of the mode it is demodulating. The nominal figure only
//! sets the discriminator's numeric scale; the decision levels are *measured*, because a
//! narrowband transmitter that is 20% under-deviated is a signal to decode, not to reject, and
//! because the same code then serves the ±1944 Hz 12.5 kHz modes and the ±1050 Hz 6.25 kHz ones.
//!
//! **All three estimates are learned only while a carrier is present.** DMR is TDMA: a radio
//! keys off for the 30 ms of every 60 ms frame that belongs to the other timeslot, and the
//! other modes key off between transmissions. A discriminator fed a dead channel emits noise
//! that swings an order of magnitude past any symbol, and loops that integrated it would arrive
//! at the next burst having learned the receiver's noise floor instead of the transmitter: the
//! clock dragged off by percent, the centre averaged halfway to zero, the level latched onto a
//! noise spike. The gate is what makes a burst mode decode at all, and a continuously-keyed
//! mode never notices it.

use num_complex::Complex;

use crate::{FmDemod, RealDecimator, SymbolSync, fir::design_rrc, iir::one_pole_coeff};

/// Symbol periods the centre estimate averages over. A receiver frequency error is static, so
/// this is deliberately far longer than any run of one symbol a mode can transmit — the tail of
/// a DMR sync pattern is 24 symbols of alternating ±3, a P25 status symbol run far fewer.
const CENTRE_SYMBOLS: f32 = 150.0;

/// Decay of the peak estimate, in symbols carrying a signal. Fast enough to follow a fading
/// transmitter, slow enough that a run without an outer symbol does not shrink the eye.
const PEAK_SYMBOLS: f32 = 60.0;

/// How much of a rise the peak estimate takes per symbol. A discriminator click, or the tail
/// of a keying edge that got past the gate, is one symbol at two or three times any level the
/// transmitter used; a plain maximum would scale the whole eye down by that for as long as its
/// decay, and an outer symbol scaled below the decision level slices as an inner one — the one
/// error the four levels cannot absorb. Following a rise over a few symbols keeps a real change
/// in level and leaves a spike behind. Under-reading the level is safe, over-reading is not.
const PEAK_ATTACK: f32 = 0.125;

/// Matched-filter span either side of the pulse, in symbol periods.
const MATCHED_SPAN: usize = 8;

/// Timing loop bandwidth in cycles per symbol. A transmitter's symbol clock is crystal-derived
/// and drifts by parts per million, so the loop only has to acquire — quickly enough to be
/// locked before a 30 ms burst's sync pattern arrives, and no faster.
const TIMING_LOOP_BW: f64 = 0.01;

/// Smoothing of the channel power the carrier gate reads, in seconds. Half a symbol at the
/// fastest mode here, and deliberately short: the gate has to fall below its threshold within
/// the matched filter's group delay of a transmitter keying down, or the filter's output is
/// still being learned from after the burst it came from has ended. Noise cannot open the gate
/// however much this jitters, because opening also takes a whole filter span of it.
const ENVELOPE_TAU_S: f64 = 1e-4;

/// How fast the gate's noise-floor estimate follows the channel, in seconds — while nothing is
/// keyed, and only then. A floor that went on learning through a transmission would climb to
/// the signal and gate out the very mode that never stops transmitting.
const FLOOR_TAU_S: f64 = 2e-2;

/// Multiples of [`FLOOR_TAU_S`] the floor is measured over before the gate may open at all.
/// Held shut rather than open: a discriminator handed a dead channel swings an order of
/// magnitude past any symbol, so a gate that counted the startup transient as a carrier would
/// hand the level estimate the noise, and the first real burst would spend itself decaying
/// back down. Nothing is lost by waiting — there is no estimate worth making yet either.
const FLOOR_SETTLE: f64 = 4.0;

/// How far above the noise floor the channel power has to sit for a carrier to be counted
/// present. Six decibels: no 4FSK signal decodes at less, so nothing that could have been
/// decoded is gated out, and power smoothed over [`ENVELOPE_TAU_S`] of noise never reaches it.
const CARRIER_RISE: f32 = 4.0;

/// The four levels a symbol is sliced to, as the dibits the modes read: +3 is 01, +1 is 00,
/// −1 is 10, −3 is 11 (ETSI TS 102 361-1 §4.2.2, and the same in TIA-102 and the M17 spec).
#[must_use]
pub fn slice(symbol: f32) -> u8 {
    match symbol {
        s if s >= 2.0 => 0b01,
        s if s >= 0.0 => 0b00,
        s if s >= -2.0 => 0b10,
        _ => 0b11,
    }
}

/// Soft values for the two bits a symbol carries, for [`crate::fec::conv`] above: the first bit
/// says the level is negative, the second that it is an outer one. Both are the distance to the
/// decision boundary, scaled so a clean symbol reaches full confidence and clipped there — an
/// over-deviated burst must not out-vote the rest of the frame.
#[must_use]
pub fn soft_bits(symbol: f32) -> [i16; 2] {
    let scale = f32::from(crate::fec::conv::CONFIDENT) / 2.0;
    let clip = |v: f32| (v * scale).clamp(-64.0, 64.0) as i16;
    [clip(-symbol), clip(symbol.abs() - 2.0)]
}

/// The symbol level a dibit is transmitted at, in the same ±1/±3 units [`Fsk4Demod`] produces.
#[must_use]
pub fn level(dibit: u8) -> f32 {
    match dibit & 0b11 {
        0b01 => 3.0,
        0b00 => 1.0,
        0b10 => -1.0,
        _ => -3.0,
    }
}

/// Four-level FSK demodulator producing normalised soft symbols.
pub struct Fsk4Demod {
    demod: FmDemod,
    matched: RealDecimator,
    sync: SymbolSync,
    centre: f32,
    /// Level the four decision thresholds are scaled by, learned only from a keyed channel.
    peak: f32,
    /// The same estimate for a channel with nothing on it, learned only while there is not.
    /// A discriminator fed noise swings further than any symbol a transmitter sends, so
    /// scaling dead time by the *signal's* level would slice every sample of it to an outer
    /// one — a stream of two dibits where there should be four, which a sync pattern then
    /// matches by chance far more often than noise ever may.
    idle_peak: f32,
    centre_alpha: f32,
    peak_decay: f32,
    envelope: f32,
    floor: f32,
    envelope_alpha: f32,
    floor_alpha: f32,
    /// Samples left of the window the floor is measured over, during which the gate is held
    /// shut because nothing is yet known about the channel.
    settling: usize,
    settle_samples: usize,
    /// Consecutive input samples whose power was above the gate's floor, saturating at
    /// `matched_taps`.
    keyed: usize,
    /// Span of the matched filter. Its output only stops carrying a burst edge once its whole
    /// support has a carrier under it, so the gate opens that late and closes that early —
    /// eroding the keyed interval by the filter's group delay at each end.
    matched_taps: usize,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
    /// Whether each sample of `filtered` has a carrier under it, and whether it has had one
    /// for the whole of the matched filter's span. The first says which of the two level
    /// estimates describes it; only the second may be *learned* from, because within a span of
    /// a keying edge the filter's output is part burst and part dead channel. A burst's first
    /// symbols still have to be sliced against the transmitter's level — they are the head of
    /// the payload — so the two questions cannot share one answer.
    carrier_run: Vec<bool>,
    settled_run: Vec<bool>,
    /// The timing recovery works on complex baseband; a discriminator produces real samples,
    /// and for those Gardner's detector reduces to `(x[k] − x[k−1])·x_mid` — the same
    /// expression, with the imaginary part carried along as zero.
    centred: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
    /// The same two questions, per symbol of `retimed`.
    retimed_carrier: Vec<bool>,
    retimed_settled: Vec<bool>,
}

impl Fsk4Demod {
    /// `deviation_hz` is the outer (±3) deviation, `alpha` the excess bandwidth of the
    /// transmitter's pulse shaping — 0.2 for the C4FM modes, 0.5 for M17.
    ///
    /// # Panics
    /// If the rate does not give at least two samples per symbol.
    #[must_use]
    pub fn new(rate: f64, baud: f64, deviation_hz: f64, alpha: f64) -> Self {
        let sps = rate / baud;
        assert!(sps >= 2.0, "need at least two samples per symbol");
        let taps = design_rrc(sps, alpha, MATCHED_SPAN);
        Self {
            demod: FmDemod::new(rate, deviation_hz),
            matched_taps: taps.len(),
            matched: RealDecimator::new(&taps, 1),
            sync: SymbolSync::new(sps, TIMING_LOOP_BW),
            centre: 0.0,
            // A signal at the nominal deviation reads ±1 out of the discriminator, so the
            // outer level starts where an on-frequency, correctly-deviated transmitter is.
            peak: 1.0,
            idle_peak: 1.0,
            centre_alpha: 1.0 / (CENTRE_SYMBOLS * sps as f32),
            peak_decay: 1.0 - 1.0 / PEAK_SYMBOLS,
            envelope: 0.0,
            floor: 0.0,
            envelope_alpha: one_pole_coeff(rate, ENVELOPE_TAU_S),
            floor_alpha: one_pole_coeff(rate, FLOOR_TAU_S),
            settling: (FLOOR_SETTLE * FLOOR_TAU_S * rate) as usize,
            settle_samples: (FLOOR_SETTLE * FLOOR_TAU_S * rate) as usize,
            keyed: 0,
            demod_buf: Vec::new(),
            filtered: Vec::new(),
            carrier_run: Vec::new(),
            settled_run: Vec::new(),
            centred: Vec::new(),
            retimed: Vec::new(),
            retimed_carrier: Vec::new(),
            retimed_settled: Vec::new(),
        }
    }

    /// Demodulate a block, appending one soft symbol per recovered symbol period to `out`.
    /// Timing and level state carry across calls, so any block split gives the same symbols.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.gate(iq);
        self.demod.process(iq, &mut self.demod_buf);
        self.matched.process(&self.demod_buf, &mut self.filtered);
        self.centred.clear();
        for (&sample, &settled) in self.filtered.iter().zip(&self.settled_run) {
            if settled {
                // Per sample rather than per symbol: the timing detector is fed the centred
                // signal, so the estimate has to advance with the samples it is subtracted
                // from, whatever size the blocks arrive in.
                self.centre += self.centre_alpha * (sample - self.centre);
            }
            self.centred.push(Complex::new(sample - self.centre, 0.0));
        }

        self.retimed.clear();
        self.retimed_carrier.clear();
        self.retimed_settled.clear();
        // One `SymbolSync` call per run of constant gate state, so each recovered symbol can be
        // attributed to one. Splitting a block this way cannot change the symbols — the timing
        // state carries across calls — and a mode that never keys off makes one call per block
        // as before.
        //
        // The signal itself is passed through either way. The gate decides only what the loops
        // are allowed to learn from, never what the decoder above gets to see: a gate that
        // misjudged a channel would otherwise be able to silence a signal that was decoding.
        let mut start = 0;
        while start < self.centred.len() {
            let (carrier, settled) = (self.carrier_run[start], self.settled_run[start]);
            let mut end = start + 1;
            while end < self.centred.len()
                && self.carrier_run[end] == carrier
                && self.settled_run[end] == settled
            {
                end += 1;
            }
            let run = &self.centred[start..end];
            if settled {
                self.sync.process(run, &mut self.retimed);
            } else {
                self.sync.process_held(run, &mut self.retimed);
            }
            self.retimed_carrier.resize(self.retimed.len(), carrier);
            self.retimed_settled.resize(self.retimed.len(), settled);
            start = end;
        }

        let (mut peak, mut idle) = (self.peak, self.idle_peak);
        let carriers = self.retimed_carrier.iter().zip(&self.retimed_settled);
        for (symbol, (&carrier, &settled)) in self.retimed.iter().zip(carriers) {
            let value = symbol.re;
            // Each span is scaled by the level of what is actually on it, and neither level
            // learns from the other's span. Within a matched-filter span of a keying edge the
            // symbol belongs to the burst and is scaled by the burst's level, but nothing
            // learns from it: that is where the transient lives.
            let level = if carrier { &mut peak } else { &mut idle };
            if settled || !carrier {
                let magnitude = value.abs();
                if magnitude > *level {
                    *level += PEAK_ATTACK * (magnitude - *level);
                } else {
                    *level *= self.peak_decay;
                }
            }
            // Guard the divide: a squelched channel produces zeros, and a symbol stream of
            // NaN would poison every decoder above this one.
            let unit = *level / 3.0;
            out.push(if unit > 1e-6 { value / unit } else { 0.0 });
        }
        (self.peak, self.idle_peak) = (peak, idle);
    }

    /// Whether each input sample has a carrier under it, judged against a noise floor measured
    /// from the channel's own quiet.
    ///
    /// The floor may not be a recent *maximum*: an idle channel's loudest noise is its own
    /// noise, so nothing would ever read as quiet, and the loops would go on learning through
    /// the seconds before a call as readily as through the dead time inside one.
    ///
    /// A channel first sampled mid-transmission measures its floor on that carrier and reads
    /// keyed off until the carrier drops. That costs the estimates their chance to learn, which
    /// is what a cold start costs anyway — it cannot cost the decoder the signal.
    fn gate(&mut self, iq: &[Complex<f32>]) {
        self.carrier_run.clear();
        self.settled_run.clear();
        for sample in iq {
            self.envelope += self.envelope_alpha * (sample.norm_sqr() - self.envelope);
            self.settling = self.settling.saturating_sub(1);
            let keyed = self.settling == 0 && self.envelope > self.floor * CARRIER_RISE;
            if !keyed {
                self.floor += self.floor_alpha * (self.envelope - self.floor);
            }
            // The matched filter's output only stops carrying a burst's keying edge once its
            // whole span has a carrier under it.
            self.keyed = if keyed {
                (self.keyed + 1).min(self.matched_taps)
            } else {
                0
            };
            self.carrier_run.push(keyed);
            self.settled_run.push(self.keyed == self.matched_taps);
        }
    }

    /// Forget the timing and level estimates — the channel moved, and what this has learned
    /// describes the transmitter it just left.
    pub fn reset(&mut self) {
        self.sync.reset();
        self.centre = 0.0;
        self.peak = 1.0;
        self.idle_peak = 1.0;
        self.envelope = 0.0;
        self.floor = 0.0;
        self.settling = self.settle_samples;
        self.keyed = 0;
        self.demod_buf.clear();
        self.filtered.clear();
        self.carrier_run.clear();
        self.settled_run.clear();
        self.centred.clear();
        self.retimed.clear();
        self.retimed_carrier.clear();
        self.retimed_settled.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::{f32::consts::PI, f64::consts::TAU};

    use super::*;

    /// A C4FM transmitter: symbol impulses through the transmit half of the root-raised-cosine
    /// pair, frequency-modulated onto the carrier. Shaping matters to this test — an
    /// unshaped rectangular symbol train through a receive RRC is not a Nyquist cascade, and
    /// the inter-symbol interference that leaves is the transmitter's fault, not the clock's.
    fn modulate(dibits: &[u8], rate: f64, baud: f64, deviation_hz: f64) -> Vec<Complex<f32>> {
        let sps = rate / baud;
        let taps = design_rrc(sps, 0.2, MATCHED_SPAN);
        let mut impulses = vec![0.0f32; dibits.len() * sps as usize + taps.len()];
        for (i, &dibit) in dibits.iter().enumerate() {
            impulses[i * sps as usize] = level(dibit) / 3.0 * sps as f32;
        }
        let mut shaped = Vec::new();
        RealDecimator::new(&taps, 1).process(&impulses, &mut shaped);
        let mut phase = 0.0f64;
        shaped
            .iter()
            .map(|&s| {
                phase += TAU * f64::from(s) * deviation_hz / rate;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect()
    }

    fn dibits(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state & 3) as u8
            })
            .collect()
    }

    /// A receiver's own noise, 40 dB below a unit-magnitude carrier — what an antenna delivers
    /// when no one is transmitting. Digital silence is not that, and a gate handed it would
    /// measure a noise floor of zero and never close again.
    const NOISE: f32 = 0.01;

    fn noise(seed: u32, len: usize) -> Vec<Complex<f32>> {
        let mut rng = crate::testutil::XorShift32(seed);
        (0..len)
            .map(|_| Complex::new(rng.next_f32() * NOISE, rng.next_f32() * NOISE))
            .collect()
    }

    /// A demodulator that has already heard the channel quiet, which is how a receiver meets
    /// every transmission it was tuned to before the transmitter keyed up. The carrier gate
    /// measures its noise floor from that quiet; one that had only ever seen a carrier has
    /// nothing to tell it apart from noise, and holds its estimates instead of learning.
    fn listening(rate: f64, baud: f64, deviation_hz: f64, alpha: f64) -> Fsk4Demod {
        let mut demod = Fsk4Demod::new(rate, baud, deviation_hz, alpha);
        let mut discard = Vec::new();
        demod.process(&noise(0x1157, (rate * 0.2) as usize), &mut discard);
        demod
    }

    /// A transmitter that keys off between bursts, as a DMR radio does for the 30 ms of every
    /// 60 ms frame that belongs to the other timeslot: `on` symbols radiated, `off` symbols of
    /// dead channel, repeated. The exciter keeps shaping through the gap, so the bursts keep
    /// the pulse tails a matched filter expects.
    fn keyed(iq: &[Complex<f32>], sps: usize, on: usize, off: usize) -> Vec<Complex<f32>> {
        let span = MATCHED_SPAN * sps;
        let frame = (on + off) * sps;
        // `modulate` puts symbol 0 a shaping span into its output, so a burst's shaped waveform
        // runs from the frame's first sample to a span past its last symbol. Keying down inside
        // that would cost the burst the pulse tails a matched filter is built to expect.
        let radiated = (on - 1) * sps + 2 * span;
        // A power amplifier ramps over about a symbol. A step is a discontinuity no radio puts
        // on the air, and its transient lands on the burst's own first symbol.
        let ramp = sps;
        let floor = noise(0xbeef, iq.len());
        iq.iter()
            .zip(floor)
            .enumerate()
            .map(|(i, (&s, n))| {
                let at = i % frame;
                let gain = match at.min(radiated.saturating_sub(at)) {
                    _ if at > radiated => 0.0,
                    edge if edge >= ramp => 1.0,
                    edge => 0.5 * (1.0 - (PI * edge as f32 / ramp as f32).cos()),
                };
                // The receiver's noise is on the channel whether the transmitter is keyed or
                // not; it is what the gate has to recognise the dead time by.
                s * gain + n
            })
            .collect()
    }

    /// Errors between `got` and `sent` once the matched filter's delay is taken out, skipping
    /// the lead-in the clock and the level estimate need. The alignment is searched rather than
    /// assumed: it is a property of the filter span, not of the mode.
    fn symbol_errors(got: &[u8], sent: &[u8], skip: usize) -> (usize, usize) {
        let (delay, errors) = (0..32)
            .map(|delay| {
                let errors = got
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .filter(|&(i, s)| sent.get(i.wrapping_sub(delay)).is_none_or(|w| w != s))
                    .count();
                (delay, errors)
            })
            .min_by_key(|&(_, errors)| errors)
            .unwrap();
        assert!((1..24).contains(&delay), "implausible alignment {delay}");
        (errors, got.len() - skip)
    }

    /// The whole point of the front end: symbols in, the same symbols out, at a deviation the
    /// demodulator was never told about.
    #[test]
    fn recovers_symbols_at_an_unexpected_deviation() {
        for deviation in [1_944.0, 1_400.0, 2_600.0] {
            let sent = dibits(400, 17);
            let iq = modulate(&sent, 48_000.0, 4_800.0, deviation);
            let mut demod = listening(48_000.0, 4_800.0, 1_944.0, 0.2);
            let mut symbols = Vec::new();
            demod.process(&iq, &mut symbols);
            let got: Vec<u8> = symbols.iter().copied().map(slice).collect();
            let (errors, total) = symbol_errors(&got, &sent, 40);
            assert!(total > 300, "only {total} symbols at {deviation} Hz");
            assert_eq!(errors, 0, "symbol errors at {deviation} Hz deviation");
        }
    }

    /// A receiver is never exactly on frequency. The centre estimate has to absorb the offset
    /// a mistuned dial or a drifting transmitter puts on the discriminator.
    #[test]
    fn tracks_a_carrier_offset() {
        let sent = dibits(600, 5);
        let mut iq = modulate(&sent, 48_000.0, 4_800.0, 1_944.0);
        // 400 Hz off — a fifth of the outer deviation, which un-centred would slice the
        // +1 level as +3 whenever it drifted high.
        for (k, s) in iq.iter_mut().enumerate() {
            *s *= Complex::from_polar(1.0, (TAU * 400.0 * k as f64 / 48_000.0) as f32);
        }
        let mut demod = listening(48_000.0, 4_800.0, 1_944.0, 0.2);
        let mut symbols = Vec::new();
        demod.process(&iq, &mut symbols);
        let got: Vec<u8> = symbols.iter().copied().map(slice).collect();
        // The centre estimate averages over hundreds of symbols, so the offset is only fully
        // absorbed in the tail — which is what a decoder hunting a sync pattern needs.
        let (errors, _) = symbol_errors(&got, &sent, symbols.len() - 200);
        assert_eq!(errors, 0);
    }

    /// The host hands a channel whatever the device gave it. Every piece of state here — the
    /// filter history, the timing accumulator, the centre and peak estimates — has to advance
    /// with the samples rather than with the calls, or the same signal would decode differently
    /// depending on the radio's buffer size.
    #[test]
    fn block_splits_do_not_change_the_symbols() {
        let sent = dibits(300, 41);
        let iq = modulate(&sent, 48_000.0, 4_800.0, 1_944.0);
        let mut whole = Vec::new();
        listening(48_000.0, 4_800.0, 1_944.0, 0.2).process(&iq, &mut whole);

        let mut demod = listening(48_000.0, 4_800.0, 1_944.0, 0.2);
        let mut ragged = Vec::new();
        let mut pos = 0;
        for len in [997usize, 1, 4_096, 65, 7].iter().cycle() {
            if pos >= iq.len() {
                break;
            }
            let end = (pos + len).min(iq.len());
            demod.process(&iq[pos..end], &mut ragged);
            pos = end;
        }
        assert_eq!(whole, ragged);
    }

    /// The TDMA case, which is what the carrier gate exists for. A DMR radio radiates 132
    /// symbols in every 288 and the receiver hears its own noise for the rest; the clock,
    /// centre and level have to arrive at each burst holding what the *transmitter* taught
    /// them, not what the dead channel did. Ungated, the discriminator noise in the gaps drags
    /// the Gardner loop percent off nominal — enough to slip whole symbols across one burst —
    /// and no sync in any of these modes survives that.
    #[test]
    fn a_keyed_transmitter_does_not_lose_its_clock_in_the_dead_time() {
        const SPS: usize = 10;
        let sent = dibits(2_880, 23);
        let iq = keyed(
            &modulate(&sent, 48_000.0, 4_800.0, 1_944.0),
            SPS,
            132,
            288 - 132,
        );
        let mut demod = listening(48_000.0, 4_800.0, 1_944.0, 0.2);
        let mut symbols = Vec::new();
        demod.process(&iq, &mut symbols);

        // One symbol per symbol period of input, gaps included: the decoders above count the
        // dead time out in symbols to find the next burst in their slot, so a clock that ran
        // fast or slow through it would put them on the wrong bits.
        let ideal = iq.len() / SPS;
        assert!(
            (symbols.len() as i64 - ideal as i64).abs() <= 2,
            "recovered {} symbols, ideal {ideal}",
            symbols.len()
        );

        // Only the keyed symbols carry anything; the last burst is checked because it is the
        // one that has been through every gap.
        let got: Vec<u8> = symbols.iter().copied().map(slice).collect();
        let (delay, _) = (0..32)
            .map(|delay| {
                let errors = (300usize..400)
                    .filter(|&i| sent.get(i.wrapping_sub(delay)).is_none_or(|w| *w != got[i]))
                    .count();
                (delay, errors)
            })
            .min_by_key(|&(_, errors)| errors)
            .unwrap();
        let last = sent.len() - 288 + delay;
        let bad: Vec<usize> = (last..last + 132)
            .filter(|&i| sent.get(i - delay).is_none_or(|w| *w != got[i]))
            .map(|i| i - last)
            .collect();
        assert!(
            bad.is_empty(),
            "symbol errors at {bad:?} in the last of {} bursts",
            sent.len() / 288
        );
    }

    #[test]
    fn a_silent_channel_produces_finite_symbols() {
        let mut demod = Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2);
        let mut symbols = Vec::new();
        demod.process(&vec![Complex::new(0.0, 0.0); 4_800], &mut symbols);
        assert!(!symbols.is_empty());
        assert!(symbols.iter().all(|s| s.is_finite()), "non-finite symbol");
    }
}
