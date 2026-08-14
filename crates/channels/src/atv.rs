//! ATV — analog television (). Envelope or discriminator → level clamp → sync
//! separator → per-line resampler → one 8-bit luma picture per field.
//!
//! The whole mode is a clock-recovery problem wearing a picture: a raster is a stream whose
//! only framing is the shape of its own blanking, so everything here hangs off classifying
//! low pulses by width. A short one is a line, a long one is a field, and a half-width one is
//! an equalizing pulse that must be ignored or every line comes out twice as fast.
//!
//! Luma only. The colour subcarrier (PAL/NTSC/SECAM alike) rides inside the video band this
//! samples and is left where it is — at the bandwidths an SDR channel gives an amateur
//! transmission, chroma would be noise on the luma rather than a second picture.

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass, flat_bandwidth_hz};
use sdrmm_modem::analog::{
    AmDemod, AmDetector, AmMode, AmParams as AmWaveform, AmRx, AngleDemod, AngleDetector,
    AngleKind, AngleParams, AngleRx,
};
use sdrmm_wire::{
    AtvModulation, AtvParams, AtvStandard, ChannelDescriptor, ChannelParams, ChannelSettings,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, VideoPicture,
    check_input_rate,
};

/// The channel's IQ rate. 2 Msps is the lowest rate that resolves a 625-line raster at all —
/// 128 samples to a line — and the highest that every receiver on the shelf can feed: an
/// RTL-SDR runs 2.048 or 2.4 Msps, both of which the DDC decimates onto this exactly. It is
/// also the ceiling on horizontal detail, which is why `bandwidth_hz` is the resolution knob
/// and the picture is as wide as the active line is long in samples.
const INPUT_RATE_HZ: f64 = 2_000_000.0;

/// Selectivity ahead of the detector. Short by this crate's standards on purpose: at 2 Msps a
/// 129-tap filter costs more than everything else in the channel put together, and 63 taps
/// still land the Blackman stopband (5.5/N ≈ 0.087 of the rate) inside Nyquist for the widest
/// band this mode admits.
const CHANNEL_TAPS: usize = 63;

/// Narrowest channel that still carries a raster: a 4.7 µs sync pulse needs roughly 200 kHz to
/// keep an edge, and below 100 kHz the separator has nothing to slice.
const MIN_BANDWIDTH_HZ: f64 = 100_000.0;

/// Slicing level between the sync tip (0.0) and peak white (1.0) — halfway to the 30 % blanking
/// level all three standards put the picture above.
const SYNC_SLICE: f32 = 0.15;

/// Sync-pulse width bounds as a fraction of the standard's nominal. The lower bound is what
/// rejects equalizing pulses, which are exactly half a sync wide.
const SYNC_MIN_FRAC: f64 = 0.65;
const SYNC_MAX_FRAC: f64 = 2.5;

/// A low pulse at least this fraction of a line long is a broad (vertical-sync) pulse. The
/// broad pulses of every standard here run past 0.4 of a line; nothing else comes close.
const BROAD_MIN_FRAC: f64 = 0.25;

/// How far from the flywheel's prediction a sync may land and still be this line's, once
/// locked. Wider and interference re-datums the line; narrower and a drifting source is lost.
const SYNC_WINDOW_FRAC: f64 = 0.08;

/// The line ends here whether or not a sync arrived — the flywheel coasting through a sync
/// the noise ate. Past [`SYNC_WINDOW_FRAC`], so a late-but-plausible sync is still taken.
const MAX_COAST_FRAC: f64 = 1.15;

/// Accepted syncs before the flywheel trusts its own prediction enough to start refusing the
/// ones that land elsewhere.
const LOCK_LINES: u8 = 4;

/// How hard a measured line length pulls the estimate, per line.
const LINE_TRACK: f64 = 0.05;
/// …and how far it may be pulled from the standard's nominal. A source that is 2 % off is
/// mistuned or mis-standard; chasing it further would let noise walk the estimate away.
const LINE_TRACK_LIMIT: f64 = 0.02;

/// Lines a vertical sync stays "the same one" for, so the several broad pulses of one group do
/// not each start a field. Longer than any group, far shorter than a field.
const VERTICAL_HOLD_LINES: u16 = 8;

/// Narrowest picture worth scanning out.
const MIN_WIDTH: u16 = 16;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "atv".to_owned(),
    name: "ATV".to_owned(),
    bandwidth_hz: 1_500_000.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    ..ChannelDescriptor::default()
});

/// One standard's raster, as fractions of a line period measured from the sync leading edge.
#[derive(Clone, Copy, Debug)]
struct Timing {
    /// Nominal horizontal sync width.
    sync: f64,
    /// Active video window, `[start, end)`.
    active: (f64, f64),
    /// Lines carrying picture, of the standard's total — the height of what this scans out.
    active_lines: u16,
    /// Lines from the leading edge of the first broad pulse to the first line carrying picture.
    /// Read off the field-blanking tables rather than derived: the equalizing, broad and
    /// post-equalizing groups that precede the picture are not a fixed share of the blanked
    /// lines, and it is the broad pulse — not the field's first line — that is detectable.
    picture_delay: u16,
}

fn timing(standard: AtvStandard) -> Timing {
    // Sync, back porch and active window in µs, straight off the standard's timing table, plus
    // the picture's height and where it starts relative to the broad pulses.
    let (sync_us, back_us, active_us, active_lines, picture_delay) = match standard {
        // CCIR System B/G: the picture starts at line 23 of the field, the broad pulses at 3.5.
        AtvStandard::Ccir625 => (4.7, 5.8, 51.95, 576, 20),
        // EIA RS-170A: the picture starts at line 21, the broad pulses at line 4.
        AtvStandard::Eia525 => (4.7, 4.5, 52.6, 480, 17),
        // System A has no equalizing group at all — the broad pulses open the field, and the
        // picture starts 14 lines later.
        AtvStandard::SystemA405 => (9.0, 5.8, 82.2, 376, 14),
    };
    // Per-line fractions rather than sample counts, so a source whose line rate is a little off
    // the standard's is resampled by *its* line and not by the nominal one.
    let line_us = 1e6 / standard.line_rate_hz();
    let start = (sync_us + back_us) / line_us;
    Timing {
        sync: sync_us / line_us,
        active: (start, start + active_us / line_us),
        active_lines,
        picture_delay,
    }
}

/// Peak tracker over the demodulated video: fast onto a new extreme, slow off it. The two ends
/// are the sync tip and peak white, which is the only absolute reference an analog raster
/// carries — every level below is measured against them.
#[derive(Clone, Debug)]
struct Levels {
    lo: f32,
    hi: f32,
    attack: f32,
    decay: f32,
    primed: bool,
}

impl Levels {
    fn new(rate: f64) -> Self {
        Self {
            lo: 0.0,
            hi: 1.0,
            // ~2 µs onto an extreme: a sync tip is 4.7 µs, so the tracker reaches it inside one.
            attack: (1.0 - (-1.0 / (rate * 2e-6)).exp()) as f32,
            // ~50 ms off it, which is 800 lines — long enough that a white-free picture does not
            // pump, short enough to follow a fade.
            decay: (1.0 / (rate * 0.05)) as f32,
            primed: false,
        }
    }

    /// Track `v` and return it normalized so the sync tip reads 0.0 and peak white 1.0.
    fn normalize(&mut self, v: f32) -> f32 {
        if !self.primed {
            self.primed = true;
            self.lo = v;
            self.hi = v + 1e-3;
        }
        let coeff = |toward_extreme: bool| {
            if toward_extreme {
                self.attack
            } else {
                self.decay
            }
        };
        self.lo += (v - self.lo) * coeff(v < self.lo);
        self.hi += (v - self.hi) * coeff(v > self.hi);
        // A carrier that has not been modulated yet collapses the two together; a floor here
        // keeps the slicer finite rather than letting it divide by nothing.
        let span = (self.hi - self.lo).max(1e-6);
        (v - self.lo) / span
    }
}

/// The library detector this channel is an attachment to, in the one shape `sdrmm_modem::analog`
/// offers it: the bare detector, with the engine's predetection filter, audio lowpass and DC
/// block all switched off. A raster is not audio — the host runtime supplies the selectivity, the
/// video band *is* the sample rate, and the blanking level a DC blocker would remove is precisely
/// the datum the sync separator slices against.
enum Detector {
    /// Amplitude television, read as an envelope.
    Envelope(AmDemod),
    /// Frequency television, read as an instantaneous frequency. A discriminator's scale cancels
    /// in the level tracker, so the deviation only has to keep the output near unity rather than
    /// match the transmitter's.
    Discriminator(AngleDemod),
}

impl Detector {
    fn new(p: &AtvParams) -> Self {
        let bandwidth = p.bandwidth_hz / 2.0 / INPUT_RATE_HZ;
        match p.modulation {
            AtvModulation::Fm => Self::Discriminator(AngleDemod::new(
                &AngleParams::new(
                    AngleKind::Fm {
                        deviation: bandwidth,
                    },
                    bandwidth,
                ),
                &AngleRx::detector_only(AngleDetector::Discriminator),
            )),
            AtvModulation::Am => Self::Envelope(AmDemod::new(
                &AmWaveform::new(AmMode::FullCarrier { depth: 1.0 }, bandwidth),
                &AmRx::detector_only(AmDetector::Envelope),
            )),
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], video: &mut Vec<f32>) {
        match self {
            Self::Envelope(am) => am.process(iq, video),
            Self::Discriminator(fm) => fm.process(iq, video),
        }
    }
}

pub struct AtvChannel {
    params: AtvParams,
    timing: Timing,
    /// Samples per line the standard asks for, and what the tracker currently believes.
    nominal_line: f64,
    line_len: f64,
    /// Demodulated video, one sample per input sample; reused across blocks.
    video: Vec<f32>,
    detector: Detector,
    /// −1.0 when the transmission keys sync at the *top* of the demodulated signal
    /// (negative-modulation AM), so everything below sees a video whose minimum is the sync tip.
    polarity: f32,
    levels: Levels,
    /// The current line, index 0 at its accepted sync leading edge.
    line: Vec<f32>,
    in_sync: bool,
    low_run: u32,
    /// Index in [`AtvChannel::line`] where the low pulse being measured began.
    pulse_start: usize,
    /// Consecutive lines whose sync landed where the flywheel predicted, capped at [`LOCK_LINES`].
    lock: u8,
    in_vertical: bool,
    vertical_hold: u16,
    /// Lines since the vertical sync — where in the field the current line sits.
    field_row: u16,
    /// Row offset of the field being written: 1 for the half-line-offset field of an interlaced
    /// source, 0 otherwise.
    parity: u16,
    /// Rows written since the field started. A field that wrote none scans out nothing.
    written: u32,
    frame: Vec<u8>,
    width: u16,
    height: u16,
}

fn params(settings: &ChannelSettings) -> Result<&AtvParams, ChannelError> {
    match &settings.params {
        ChannelParams::Atv(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "atv channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_bandwidth(p: &AtvParams) -> Result<(), ChannelError> {
    let widest = flat_bandwidth_hz(INPUT_RATE_HZ);
    if p.bandwidth_hz.is_finite() && (MIN_BANDWIDTH_HZ..=widest).contains(&p.bandwidth_hz) {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "atv bandwidth must be in [{MIN_BANDWIDTH_HZ}, {widest}] Hz, got {}",
            p.bandwidth_hz
        )))
    }
}

pub(crate) fn channel_filter(p: &AtvParams) -> Result<ChannelFilter, ChannelError> {
    check_bandwidth(p)?;
    let cutoff = p.bandwidth_hz / 2.0 / INPUT_RATE_HZ;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, cutoff),
        1,
    )))
}

/// The band an ATV channel occupies, symmetric about its carrier. A broadcast transmission is
/// vestigial-sideband and therefore not symmetric, but a symmetric selection about the carrier
/// is what an envelope detector wants either way; what it costs is a lift below the vestige's
/// corner, which reads as a soft low-frequency contrast boost and not as a lost picture.
pub(crate) fn occupied_band(p: &AtvParams) -> (f64, f64) {
    (-p.bandwidth_hz / 2.0, p.bandwidth_hz / 2.0)
}

impl AtvChannel {
    fn configure(&mut self, p: &AtvParams) -> Result<(), ChannelError> {
        check_bandwidth(p)?;
        let timing = timing(p.standard);
        let nominal_line = INPUT_RATE_HZ / p.standard.line_rate_hz();
        let width = ((timing.active.1 - timing.active.0) * nominal_line).round();
        let width = if width >= f64::from(MIN_WIDTH) {
            width as u16
        } else {
            return Err(ChannelError::InvalidSettings(format!(
                "{} lines leave only {width} samples of active video at {INPUT_RATE_HZ} Hz",
                p.standard.lines()
            )));
        };
        self.timing = timing;
        self.nominal_line = nominal_line;
        self.line_len = nominal_line;
        self.width = width;
        self.height = timing.active_lines;
        self.frame.clear();
        self.frame
            .resize(usize::from(width) * usize::from(timing.active_lines), 0);
        // A discriminator's scale cancels in the level tracker, so the deviation only has to
        // keep the output near unity rather than match the transmitter's.
        self.detector = Detector::new(p);
        // AM television is negative-modulated — peak carrier is the sync tip — so its envelope
        // arrives upside down; FM ATV keys the other way. `invert` flips whichever applies.
        let flipped = matches!(p.modulation, AtvModulation::Am) != p.invert;
        self.polarity = if flipped { -1.0 } else { 1.0 };
        self.params = p.clone();
        self.restart();
        Ok(())
    }

    /// Drop everything tied to the raster being scanned — the sync hunt starts over, and no
    /// half-written picture from the old geometry escapes.
    fn restart(&mut self) {
        self.line.clear();
        self.in_sync = false;
        self.low_run = 0;
        self.pulse_start = 0;
        self.lock = 0;
        self.in_vertical = false;
        self.vertical_hold = 0;
        self.field_row = 0;
        self.parity = 0;
        self.written = 0;
        self.frame.fill(0);
    }

    /// Rows between one field's lines in the frame: interlaced fields land on alternate rows.
    fn row_step(&self) -> u32 {
        if self.params.interlace { 2 } else { 1 }
    }

    fn push_sample(&mut self, v: f32, out: &mut ChannelOutputs) {
        let n = self.levels.normalize(v);
        if n < SYNC_SLICE {
            if self.in_sync {
                self.low_run += 1;
            } else {
                self.in_sync = true;
                self.low_run = 1;
                self.pulse_start = self.line.len();
            }
        } else if self.in_sync {
            self.in_sync = false;
            self.classify(out);
        }
        self.line.push(n);
        if self.line.len() as f64 >= self.line_len * MAX_COAST_FRAC {
            // No sync arrived where one was due: end the line on the flywheel's own count so a
            // burst of noise costs one torn line instead of the rest of the field.
            let len = self.line_len.round() as usize;
            self.end_line(len, out);
            self.lock = self.lock.saturating_sub(1);
        }
    }

    /// A low pulse just ended: decide what it was and, if it was this line's sync, close the
    /// line on it.
    fn classify(&mut self, out: &mut ChannelOutputs) {
        let start = self.pulse_start;
        let width = f64::from(self.low_run);
        if width >= self.line_len * BROAD_MIN_FRAC {
            self.vertical(start, out);
            return;
        }
        let nominal = self.timing.sync * self.line_len;
        if width < nominal * SYNC_MIN_FRAC || width > nominal * SYNC_MAX_FRAC {
            // An equalizing pulse or a noise notch. The flywheel keeps time through it.
            return;
        }
        let measured = start as f64;
        if self.lock >= LOCK_LINES
            && (measured - self.line_len).abs() > self.line_len * SYNC_WINDOW_FRAC
        {
            return;
        }
        if measured < self.line_len * 0.5 {
            // Far too soon to be the next line's sync — the hunt landed mid-line. Re-datum on
            // it without scanning out the fragment before it.
            self.line.drain(..start);
            return;
        }
        // The measured length is the truth about this source's line rate; let it pull the
        // estimate, bounded so noise cannot walk it off the standard.
        let pulled = self.line_len + (measured - self.line_len) * LINE_TRACK;
        let limit = self.nominal_line * LINE_TRACK_LIMIT;
        self.line_len = pulled.clamp(self.nominal_line - limit, self.nominal_line + limit);
        self.end_line(start, out);
        self.lock = (self.lock + 1).min(LOCK_LINES);
    }

    /// Scan the first `len` samples of the buffer out as a row and drop them.
    fn end_line(&mut self, len: usize, out: &mut ChannelOutputs) {
        let len = len.min(self.line.len());
        self.write_row(len);
        self.line.drain(..len);
        self.field_row = self.field_row.saturating_add(1);
        if self.vertical_hold > 0 {
            self.vertical_hold -= 1;
            if self.vertical_hold == 0 {
                self.in_vertical = false;
            }
        }
        // A source whose vertical sync never arrives (or is unreadable) would otherwise write
        // the same rows forever; rolling the field over keeps a picture moving instead.
        if self.field_row > self.timing.active_lines + self.timing.picture_delay {
            self.finish_field(out);
            self.field_row = 0;
        }
    }

    /// Resample the active window of `line[..len]` into the frame row this line belongs to.
    fn write_row(&mut self, len: usize) {
        // Nothing reaches the picture until the line clock is locked. An unlocked flywheel is
        // free-running at the standard's nominal rate over whatever is on the channel, and what
        // it would scan out is a raster this code invented rather than one anybody transmitted.
        if self.lock < LOCK_LINES
            || self.field_row < self.timing.picture_delay
            || len < MIN_WIDTH as usize
        {
            return;
        }
        let row = u32::from(self.field_row - self.timing.picture_delay);
        let dest = row * self.row_step() + u32::from(self.parity);
        if dest >= u32::from(self.height) {
            return;
        }
        let span = len as f64;
        // Black comes off this line's own back porch rather than from the assumed 30 % blanking
        // level: a clamp per line is what holds the brightness steady through a fade, and it is
        // free here because the samples are already in hand.
        let (mut b0, mut b1) = (self.timing.sync * span, self.timing.active.0 * span);
        let inset = (b1 - b0) * 0.25;
        b0 += inset;
        b1 -= inset;
        let black = mean(&self.line[b0 as usize..(b1 as usize).max(b0 as usize + 1)]);
        // Peak white is 1.0 by construction of the level tracker, so what is left above black
        // is the whole picture range; a collapsed one falls back to the standard 70 %.
        let range = if 1.0 - black > 0.05 { 1.0 - black } else { 0.7 };

        let start = self.timing.active.0 * span;
        let active = (self.timing.active.1 - self.timing.active.0) * span;
        let base = usize::from(self.width) * dest as usize;
        for k in 0..usize::from(self.width) {
            let x = start + (k as f64 + 0.5) * active / f64::from(self.width);
            let i = x as usize;
            let v = if i + 1 < len {
                let f = (x - i as f64) as f32;
                self.line[i] + (self.line[i + 1] - self.line[i]) * f
            } else {
                self.line[len - 1]
            };
            self.frame[base + k] = (((v - black) / range).clamp(0.0, 1.0) * 255.0) as u8;
        }
        self.written += 1;
    }

    /// A broad pulse: the field this line belongs to has just started.
    fn vertical(&mut self, start: usize, out: &mut ChannelOutputs) {
        self.vertical_hold = VERTICAL_HOLD_LINES;
        if self.in_vertical {
            return;
        }
        self.in_vertical = true;
        // Interlace is a half-line offset and nothing else: the second field's vertical sync
        // arrives half a line late, which is the whole of what tells the two apart.
        let phase = (start as f64 / self.line_len).rem_euclid(1.0);
        self.parity = u16::from(self.params.interlace && (0.25..0.75).contains(&phase));
        self.finish_field(out);
        self.field_row = 0;
    }

    /// Hand over the frame as it now stands, if this field put anything into it.
    fn finish_field(&mut self, out: &mut ChannelOutputs) {
        if self.written == 0 {
            return;
        }
        self.written = 0;
        // The one allocation on this path, and the same bounded deviation from  the PCM
        // hand-off takes: a picture per field is 50 a second, and the alternative is a pool the
        // host would have to hand back on a thread it does not own.
        out.video.push(VideoPicture {
            width: self.width,
            height: self.height,
            luma: self.frame.clone(),
        });
    }
}

fn mean(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f32>() / samples.len() as f32
}

impl ChannelRx for AtvChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        let mut chan = Self {
            params: p.clone(),
            timing: timing(p.standard),
            nominal_line: 0.0,
            line_len: 0.0,
            video: Vec::new(),
            detector: Detector::new(p),
            polarity: 1.0,
            levels: Levels::new(INPUT_RATE_HZ),
            line: Vec::new(),
            in_sync: false,
            low_run: 0,
            pulse_start: 0,
            lock: 0,
            in_vertical: false,
            vertical_hold: 0,
            field_row: 0,
            parity: 0,
            written: 0,
            frame: Vec::new(),
            width: 0,
            height: 0,
        };
        chan.configure(p)?;
        Ok(chan)
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        // Geometry, polarity and detector all change the meaning of the raster mid-scan, so the
        // hunt restarts rather than splicing the new standard onto the old field.
        self.configure(p)
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        // Taken out so the per-sample loop can call back into `self`; put back below, so the
        // buffer's capacity survives the block and nothing here allocates in steady state.
        let mut video = std::mem::take(&mut self.video);
        self.detector.process(iq, &mut video);
        let polarity = self.polarity;
        for &v in &video {
            self.push_sample(v * polarity, out);
        }
        self.video = video;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::NfmParams;

    use super::*;
    use crate::{
        testgen::atv::{AtvSource, BAR_LEVELS, bars},
        testutil::settings,
    };

    fn channel(p: AtvParams) -> AtvChannel {
        AtvChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Atv(p)),
        )
        .unwrap()
    }

    /// Run `iq` through in ragged blocks (the sizes a device really hands over) and collect
    /// every picture that came out.
    fn run(chan: &mut AtvChannel, iq: &[Complex<f32>]) -> Vec<VideoPicture> {
        let mut out = ChannelOutputs::default();
        let mut pictures = Vec::new();
        let mut at = 0;
        for (k, size) in [4_096usize, 1_000, 65_536, 777]
            .iter()
            .copied()
            .cycle()
            .enumerate()
        {
            let _ = k;
            if at >= iq.len() {
                break;
            }
            let end = (at + size).min(iq.len());
            out.reset();
            chan.process(&iq[at..end], &mut out);
            pictures.append(&mut out.video);
            at = end;
        }
        pictures
    }

    /// Mean luma of the middle of the `bar`-th vertical bar on `row`.
    fn bar_luma(picture: &VideoPicture, row: usize, bar: usize) -> f64 {
        let bars = BAR_LEVELS.len();
        let w = usize::from(picture.width);
        let lo = w * bar / bars;
        let hi = w * (bar + 1) / bars;
        // Bar edges smear over the channel filter's rise time; measure the settled middle.
        let inset = (hi - lo) / 4;
        let span = &picture.luma[row * w + lo + inset..row * w + hi - inset];
        span.iter().map(|&v| f64::from(v)).sum::<f64>() / span.len() as f64
    }

    fn params_for(standard: AtvStandard, modulation: AtvModulation) -> AtvParams {
        AtvParams {
            modulation,
            standard,
            ..AtvParams::default()
        }
    }

    /// The mode's whole claim: a standards-timed bar pattern comes back as a picture of the
    /// right geometry with the bars at the right places and the right brightnesses.
    #[test]
    fn decodes_a_bar_pattern_from_a_ccir_625_transmission() {
        let p = params_for(AtvStandard::Ccir625, AtvModulation::Am);
        let iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 6);
        let mut chan = channel(p);
        let pictures = run(&mut chan, &iq);

        // Six frames in, both fields of at least two frames must have been scanned out.
        assert!(pictures.len() >= 8, "pictures {}", pictures.len());
        let picture = pictures.last().expect("at least one picture");
        assert_eq!((picture.width, picture.height), (104, 576));
        assert_eq!(
            picture.luma.len(),
            usize::from(picture.width) * usize::from(picture.height)
        );

        // Rows well inside the picture, one from each interlaced field, so the weave is proven
        // and not just the first field.
        for row in [200usize, 201, 400] {
            for (bar, &level) in BAR_LEVELS.iter().enumerate() {
                let got = bar_luma(picture, row, bar);
                let want = f64::from(level) * 255.0;
                assert!(
                    (got - want).abs() < 26.0,
                    "row {row} bar {bar}: luma {got:.0}, expected {want:.0}"
                );
            }
        }
    }

    /// The bars must be monotonically brighter left to right in every standard and either
    /// modulation — which is the assertion that catches an inverted polarity, a half-line
    /// horizontal offset, or a resampler reading the wrong window.
    #[test]
    fn every_standard_and_modulation_scans_the_bars_in_order() {
        for standard in [
            AtvStandard::Ccir625,
            AtvStandard::Eia525,
            AtvStandard::SystemA405,
        ] {
            for modulation in [AtvModulation::Am, AtvModulation::Fm] {
                let p = params_for(standard, modulation);
                let iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 4);
                let mut chan = channel(p);
                let pictures = run(&mut chan, &iq);
                let picture = pictures
                    .last()
                    .unwrap_or_else(|| panic!("{standard:?}/{modulation:?} produced no picture"));
                assert_eq!(picture.height, timing(standard).active_lines);
                let row = usize::from(picture.height) / 2;
                let luma: Vec<f64> = (0..BAR_LEVELS.len())
                    .map(|bar| bar_luma(picture, row, bar))
                    .collect();
                for pair in luma.windows(2) {
                    assert!(
                        pair[1] > pair[0] + 10.0,
                        "{standard:?}/{modulation:?}: bars not ascending: {luma:?}"
                    );
                }
                assert!(
                    luma[0] < 40.0,
                    "{standard:?}/{modulation:?}: black {luma:?}"
                );
                assert!(
                    *luma.last().unwrap() > 215.0,
                    "{standard:?}/{modulation:?}: white {luma:?}"
                );
            }
        }
    }

    /// `invert` is what an operator reaches for when the picture comes back as a negative, so
    /// inverting the transmission *and* the setting must land back on the same picture.
    #[test]
    fn invert_undoes_a_reversed_transmission() {
        let mut p = params_for(AtvStandard::Ccir625, AtvModulation::Am);
        p.invert = true;
        let iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 4);
        let mut chan = channel(p);
        let picture = run(&mut chan, &iq).pop().expect("a picture");
        let row = usize::from(picture.height) / 2;
        assert!(bar_luma(&picture, row, 0) < 40.0);
        assert!(bar_luma(&picture, row, BAR_LEVELS.len() - 1) > 215.0);
    }

    /// A progressive source has no half-line offset, so every line must land on consecutive
    /// rows — read back as a picture with no blank alternate rows.
    #[test]
    fn progressive_sources_fill_every_row() {
        let mut p = params_for(AtvStandard::Ccir625, AtvModulation::Am);
        p.interlace = false;
        let iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 4);
        let mut chan = channel(p);
        let picture = run(&mut chan, &iq).pop().expect("a picture");
        let white = BAR_LEVELS.len() - 1;
        for row in [100usize, 101, 102, 103] {
            assert!(
                bar_luma(&picture, row, white) > 215.0,
                "row {row} of a progressive scan is blank"
            );
        }
    }

    /// Noise must cost lines, not the lock: the flywheel exists so a burst that eats a sync
    /// leaves the picture standing.
    #[test]
    fn a_noisy_channel_still_scans_a_recognizable_picture() {
        let p = params_for(AtvStandard::Ccir625, AtvModulation::Am);
        let mut iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 4);
        crate::testgen::add_noise(&mut iq, 0xA7C3, 0.08);
        let mut chan = channel(p);
        let picture = run(&mut chan, &iq).pop().expect("a picture");
        let row = usize::from(picture.height) / 2;
        let black = bar_luma(&picture, row, 0);
        let white = bar_luma(&picture, row, BAR_LEVELS.len() - 1);
        assert!(white - black > 150.0, "contrast {black:.0}..{white:.0}");
    }

    /// Silence must not scan out a picture of noise: with no sync there is no raster, and a
    /// panel showing snow it invented is worse than a panel showing nothing.
    #[test]
    fn an_empty_channel_produces_no_picture() {
        let p = params_for(AtvStandard::Ccir625, AtvModulation::Am);
        let mut chan = channel(p);
        let quiet = vec![Complex::new(0.0, 0.0); 400_000];
        assert!(run(&mut chan, &quiet).is_empty());
    }

    #[test]
    fn apply_changes_the_standard_and_the_geometry_with_it() {
        let mut chan = channel(params_for(AtvStandard::Ccir625, AtvModulation::Am));
        assert_eq!((chan.width, chan.height), (104, 576));
        let p = params_for(AtvStandard::SystemA405, AtvModulation::Fm);
        chan.apply(settings(ChannelParams::Atv(p.clone()))).unwrap();
        assert_eq!(chan.height, 376);
        let iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 4);
        let picture = run(&mut chan, &iq).pop().expect("a picture");
        assert_eq!((picture.width, picture.height), (chan.width, 376));
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(AtvParams::default());
        assert!(matches!(
            chan.apply(settings(ChannelParams::Nfm(NfmParams::default()))),
            Err(ChannelError::InvalidSettings(_))
        ));
    }

    #[test]
    fn out_of_range_bandwidth_is_rejected() {
        for bandwidth_hz in [0.0, f64::NAN, 50_000.0, 1_900_000.0] {
            let p = AtvParams {
                bandwidth_hz,
                ..AtvParams::default()
            };
            assert!(
                matches!(channel_filter(&p), Err(ChannelError::InvalidSettings(_))),
                "{bandwidth_hz} must be rejected"
            );
            assert!(matches!(
                AtvChannel::new(
                    ChannelCtx {
                        input_rate: INPUT_RATE_HZ
                    },
                    settings(ChannelParams::Atv(p)),
                ),
                Err(ChannelError::InvalidSettings(_))
            ));
        }
    }

    #[test]
    fn rejects_a_mismatched_input_rate() {
        assert!(matches!(
            AtvChannel::new(
                ChannelCtx {
                    input_rate: 48_000.0
                },
                settings(ChannelParams::Atv(AtvParams::default())),
            ),
            Err(ChannelError::InvalidSettings(_))
        ));
    }

    /// The heaviest per-sample path in the crate after ADS-B, and the one most able to stall a
    /// DSP thread that has other channels on it. The budget is real time in an *unoptimized*
    /// build, which this beats by roughly 3× today: a gate against the order-of-magnitude
    /// regression that would make ATV unhostable, not a benchmark.
    #[test]
    fn keeps_ahead_of_the_channel_rate() {
        let p = params_for(AtvStandard::Ccir625, AtvModulation::Am);
        let iq = bars(&AtvSource::new(&p, INPUT_RATE_HZ), 10);
        let seconds = iq.len() as f64 / INPUT_RATE_HZ;
        let mut chan = channel(p);
        let mut out = ChannelOutputs::default();
        let started = std::time::Instant::now();
        for block in iq.chunks(16_384) {
            out.reset();
            chan.process(block, &mut out);
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(
            elapsed < seconds,
            "{seconds:.2} s of video took {elapsed:.2} s"
        );
    }
}
