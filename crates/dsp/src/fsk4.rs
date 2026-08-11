//! Four-level FSK symbol recovery (PLAN §13 wave 3) — the front end every digital-voice mode
//! but D-Star shares.
//!
//! Chain: FM discriminator → root-raised-cosine matched filter → Gardner symbol clock → level
//! normalisation. What comes out is one soft symbol per symbol period, scaled so the four
//! transmitted levels sit at ±1 and ±3 whatever the transmitter's actual deviation is.
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

use num_complex::Complex;

use crate::{FmDemod, RealDecimator, SymbolSync, fir::design_rrc};

/// Symbol periods the centre estimate averages over. A receiver frequency error is static, so
/// this is deliberately far longer than any run of one symbol a mode can transmit — the tail of
/// a DMR sync pattern is 24 symbols of alternating ±3, a P25 status symbol run far fewer.
const CENTRE_SYMBOLS: f32 = 150.0;

/// Decay of the peak estimate, per symbol. Fast enough to follow a fading signal between
/// bursts, slow enough that a run without an outer symbol does not shrink the eye.
const PEAK_SYMBOLS: f32 = 60.0;

/// Matched-filter span either side of the pulse, in symbol periods.
const MATCHED_SPAN: usize = 8;

/// Timing loop bandwidth in cycles per symbol. A transmitter's symbol clock is crystal-derived
/// and drifts by parts per million, so the loop only has to acquire — quickly enough to be
/// locked before a 30 ms burst's sync pattern arrives, and no faster.
const TIMING_LOOP_BW: f64 = 0.01;

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
    peak: f32,
    centre_alpha: f32,
    peak_decay: f32,
    demod_buf: Vec<f32>,
    filtered: Vec<f32>,
    /// The timing recovery works on complex baseband; a discriminator produces real samples,
    /// and for those Gardner's detector reduces to `(x[k] − x[k−1])·x_mid` — the same
    /// expression, with the imaginary part carried along as zero.
    centred: Vec<Complex<f32>>,
    retimed: Vec<Complex<f32>>,
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
        Self {
            demod: FmDemod::new(rate, deviation_hz),
            matched: RealDecimator::new(&design_rrc(sps, alpha, MATCHED_SPAN), 1),
            sync: SymbolSync::new(sps, TIMING_LOOP_BW),
            centre: 0.0,
            // A signal at the nominal deviation reads ±1 out of the discriminator, so the
            // outer level starts where an on-frequency, correctly-deviated transmitter is.
            peak: 1.0,
            centre_alpha: 1.0 / (CENTRE_SYMBOLS * sps as f32),
            peak_decay: 1.0 - 1.0 / PEAK_SYMBOLS,
            demod_buf: Vec::new(),
            filtered: Vec::new(),
            centred: Vec::new(),
            retimed: Vec::new(),
        }
    }

    /// Demodulate a block, appending one soft symbol per recovered symbol period to `out`.
    /// Timing and level state carry across calls, so any block split gives the same symbols.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<f32>) {
        self.demod.process(iq, &mut self.demod_buf);
        self.matched.process(&self.demod_buf, &mut self.filtered);
        self.centred.clear();
        for &sample in &self.filtered {
            // Per sample rather than per symbol: the timing detector is fed the centred
            // signal, so the estimate has to advance with the samples it is subtracted from,
            // whatever size the blocks arrive in.
            self.centre += self.centre_alpha * (sample - self.centre);
            self.centred.push(Complex::new(sample - self.centre, 0.0));
        }
        self.retimed.clear();
        self.sync.process(&self.centred, &mut self.retimed);
        for symbol in &self.retimed {
            let value = symbol.re;
            self.peak = (self.peak * self.peak_decay).max(value.abs());
            // Guard the divide: a squelched channel produces zeros, and a symbol stream of
            // NaN would poison every decoder above this one.
            let unit = self.peak / 3.0;
            out.push(if unit > 1e-6 { value / unit } else { 0.0 });
        }
    }

    /// Forget the timing and level estimates — the channel moved, and what this has learned
    /// describes the transmitter it just left.
    pub fn reset(&mut self) {
        self.sync.reset();
        self.centre = 0.0;
        self.peak = 1.0;
        self.demod_buf.clear();
        self.filtered.clear();
        self.centred.clear();
        self.retimed.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

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
            let mut demod = Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2);
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
        let mut demod = Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2);
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
        Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2).process(&iq, &mut whole);

        let mut demod = Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2);
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

    #[test]
    fn a_silent_channel_produces_finite_symbols() {
        let mut demod = Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2);
        let mut symbols = Vec::new();
        demod.process(&vec![Complex::new(0.0, 0.0); 4_800], &mut symbols);
        assert!(!symbols.is_empty());
        assert!(symbols.iter().all(|s| s.is_finite()), "non-finite symbol");
    }
}
