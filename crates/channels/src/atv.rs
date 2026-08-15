//! ATV — analog television. Envelope or discriminator → level clamp → sync separator →
//! per-line composite-video decoder, with PAL/NTSC colour and an optional FM sound carrier.
//!
//! The whole mode is a clock-recovery problem wearing a picture: a raster is a stream whose
//! only framing is the shape of its own blanking, so everything here hangs off classifying
//! low pulses by width. A short one is a line, a long one is a field, and a half-width one is
//! an equalizing pulse that must be ignored or every line comes out twice as fast.
//!
use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Ddc, Decimator, Deemphasis, RealDecimator, design_lowpass, flat_bandwidth_hz};
use sdrmm_modem::analog::{
    AmDemod, AmDetector, AmMode, AmParams as AmWaveform, AmRx, AngleDemod, AngleDetector,
    AngleKind, AngleParams, AngleRx,
};
use sdrmm_wire::{
    AtvColor, AtvModulation, AtvParams, AtvStandard, ChannelDescriptor, ChannelParams,
    ChannelSettings,
};

use crate::{
    AUDIO_RATE, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, VideoPicture,
    check_input_rate, clamp_full_scale,
};

/// The minimum channel IQ rate. 2 Msps resolves a 625-line monochrome raster at 128 samples per
/// line; colour and sound retain the device's higher native rate and filter the composite video
/// internally.
const INPUT_RATE_HZ: f64 = 2_000_000.0;
const MAX_INPUT_RATE_HZ: f64 = 20_000_000.0;

/// Selectivity ahead of the detector. Short by this crate's standards on purpose: at 2 Msps a
/// 129-tap filter costs more than everything else in the channel put together, and 63 taps
/// still land the Blackman stopband (5.5/N ≈ 0.087 of the rate) inside Nyquist for the widest
/// band this mode admits.
const CHANNEL_TAPS: usize = 63;
const COLOR_BANDWIDTH_HZ: f64 = 600_000.0;
const PAL_SUBCARRIER_HZ: f64 = 4_433_618.75;
const NTSC_SUBCARRIER_HZ: f64 = 3_579_545.0;
const SOUND_IF_RATE_HZ: f64 = 240_000.0;
const SOUND_DEVIATION_HZ: f64 = 50_000.0;
const SOUND_AUDIO_HZ: f64 = 15_000.0;
const SOUND_DECIM: usize = 5;
const SOUND_TAPS: usize = 199;

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
    has_audio: true,
    has_video: true,
    native_rate_max_hz: Some(MAX_INPUT_RATE_HZ),
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
    fn new(p: &AtvParams, rate: f64) -> Self {
        let bandwidth = video_high_hz(p) / rate;
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

enum VideoFront {
    Luma(Ddc),
    Composite(Decimator),
}

impl VideoFront {
    fn new(input_rate: f64, p: &AtvParams) -> Result<(Self, f64), ChannelError> {
        let full_rate = p.color != AtvColor::Monochrome
            || (p.modulation == AtvModulation::Fm && p.sound_subcarrier_hz.is_some());
        if full_rate {
            let cutoff = video_high_hz(p) / input_rate;
            Ok((
                Self::Composite(Decimator::new(&design_lowpass(CHANNEL_TAPS, cutoff), 1)),
                input_rate,
            ))
        } else {
            let ddc = Ddc::new(input_rate, INPUT_RATE_HZ, 0.0)
                .map_err(|error| ChannelError::InvalidSettings(error.to_string()))?;
            Ok((Self::Luma(ddc), INPUT_RATE_HZ))
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        match self {
            Self::Luma(ddc) => ddc.process(iq, out),
            Self::Composite(filter) => filter.process(iq, out),
        }
    }
}

struct SoundDecoder {
    ddc: Ddc,
    discriminator: AngleDemod,
    decimator: RealDecimator,
    deemphasis: Deemphasis,
    baseband: Vec<Complex<f32>>,
    demodulated: Vec<f32>,
    real_iq: Vec<Complex<f32>>,
}

impl SoundDecoder {
    fn new(rate: f64, carrier_hz: f64, deemphasis_us: f32) -> Result<Self, ChannelError> {
        let ddc = Ddc::new(rate, SOUND_IF_RATE_HZ, carrier_hz)
            .map_err(|error| ChannelError::InvalidSettings(error.to_string()))?;
        let discriminator = AngleDemod::new(
            &AngleParams::new(
                AngleKind::Fm {
                    deviation: SOUND_DEVIATION_HZ / SOUND_IF_RATE_HZ,
                },
                SOUND_AUDIO_HZ / SOUND_IF_RATE_HZ,
            ),
            &AngleRx::detector_only(AngleDetector::Discriminator),
        );
        Ok(Self {
            ddc,
            discriminator,
            decimator: RealDecimator::new(
                &design_lowpass(SOUND_TAPS, SOUND_AUDIO_HZ / SOUND_IF_RATE_HZ),
                SOUND_DECIM,
            ),
            deemphasis: Deemphasis::new(f64::from(AUDIO_RATE), deemphasis_us),
            baseband: Vec::new(),
            demodulated: Vec::new(),
            real_iq: Vec::new(),
        })
    }

    fn process_iq(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.ddc.process(iq, &mut self.baseband);
        self.finish(out);
    }

    fn process_composite(&mut self, video: &[f32], out: &mut ChannelOutputs) {
        self.real_iq.clear();
        self.real_iq
            .extend(video.iter().map(|&sample| Complex::new(sample, 0.0)));
        self.ddc.process(&self.real_iq, &mut self.baseband);
        self.finish(out);
    }

    fn finish(&mut self, out: &mut ChannelOutputs) {
        self.discriminator
            .process(&self.baseband, &mut self.demodulated);
        self.decimator
            .process(&self.demodulated, &mut out.audio_pcm);
        self.deemphasis.process(&mut out.audio_pcm);
        clamp_full_scale(&mut out.audio_pcm);
        if !out.audio_pcm.is_empty() {
            out.audio_rate = AUDIO_RATE;
        }
    }
}

pub struct AtvChannel {
    params: AtvParams,
    input_rate: f64,
    video_rate: f64,
    timing: Timing,
    /// Samples per line the standard asks for, and what the tracker currently believes.
    nominal_line: f64,
    line_len: f64,
    /// Demodulated video, one sample per input sample; reused across blocks.
    video: Vec<f32>,
    filtered: Vec<Complex<f32>>,
    front: VideoFront,
    detector: Detector,
    sound: Option<SoundDecoder>,
    /// −1.0 when the transmission keys sync at the *top* of the demodulated signal
    /// (negative-modulation AM), so everything below sees a video whose minimum is the sync tip.
    polarity: f32,
    levels: Levels,
    sync_level: f32,
    sync_coeff: f32,
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
    rgb: Vec<u8>,
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
        if let Some(sound_hz) = p.sound_subcarrier_hz
            && !(sound_hz.is_finite() && (500_000.0..=9_000_000.0).contains(&sound_hz))
        {
            return Err(ChannelError::InvalidSettings(format!(
                "atv sound subcarrier must be in [500000, 9000000] Hz, got {sound_hz}"
            )));
        }
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
    Ok(ChannelFilter::Passthrough)
}

/// The video band is selected symmetrically about the picture carrier for envelope detection;
/// an optional sound carrier extends the upper edge independently.
pub(crate) fn occupied_band(p: &AtvParams) -> (f64, f64) {
    let video = video_high_hz(p);
    let sound = p
        .sound_subcarrier_hz
        .map_or(0.0, |carrier| carrier + SOUND_DEVIATION_HZ + SOUND_AUDIO_HZ);
    (-video, video.max(sound))
}

fn color_carrier_hz(color: AtvColor) -> Option<f64> {
    match color {
        AtvColor::Monochrome => None,
        AtvColor::Pal => Some(PAL_SUBCARRIER_HZ),
        AtvColor::Ntsc => Some(NTSC_SUBCARRIER_HZ),
    }
}

fn video_high_hz(p: &AtvParams) -> f64 {
    let chroma = color_carrier_hz(p.color).map_or(0.0, |carrier| carrier + COLOR_BANDWIDTH_HZ);
    let embedded_sound = if p.modulation == AtvModulation::Fm {
        p.sound_subcarrier_hz
            .map_or(0.0, |carrier| carrier + SOUND_DEVIATION_HZ + SOUND_AUDIO_HZ)
    } else {
        0.0
    };
    (p.bandwidth_hz / 2.0).max(chroma).max(embedded_sound)
}

impl AtvChannel {
    fn configure(&mut self, p: &AtvParams) -> Result<(), ChannelError> {
        check_bandwidth(p)?;
        let (_, high) = occupied_band(p);
        if high >= self.input_rate / 2.0 {
            return Err(ChannelError::InvalidSettings(format!(
                "atv color/sound needs more than {high:.0} Hz above the picture carrier, but the device Nyquist limit is {:.0} Hz",
                self.input_rate / 2.0
            )));
        }
        let (front, video_rate) = VideoFront::new(self.input_rate, p)?;
        let timing = timing(p.standard);
        let nominal_line = video_rate / p.standard.line_rate_hz();
        let width = ((timing.active.1 - timing.active.0) * nominal_line).round();
        let width = if width >= f64::from(MIN_WIDTH) {
            width as u16
        } else {
            return Err(ChannelError::InvalidSettings(format!(
                "{} lines leave only {width} samples of active video at {video_rate} Hz",
                p.standard.lines()
            )));
        };
        self.timing = timing;
        self.video_rate = video_rate;
        self.nominal_line = nominal_line;
        self.line_len = nominal_line;
        self.width = width;
        self.height = timing.active_lines;
        self.frame.clear();
        self.frame
            .resize(usize::from(width) * usize::from(timing.active_lines), 0);
        self.rgb.clear();
        if p.color != AtvColor::Monochrome {
            self.rgb.resize(self.frame.len() * 3, 0);
        }
        // A discriminator's scale cancels in the level tracker, so the deviation only has to
        // keep the output near unity rather than match the transmitter's.
        self.front = front;
        self.detector = Detector::new(p, video_rate);
        self.levels = Levels::new(video_rate);
        self.sync_coeff = (1.0 - (-TAU * 700_000.0 / video_rate).exp()) as f32;
        self.sound = p
            .sound_subcarrier_hz
            .map(|carrier| {
                let rate = if p.modulation == AtvModulation::Am {
                    self.input_rate
                } else {
                    video_rate
                };
                let deemphasis = if p.color == AtvColor::Ntsc {
                    75.0
                } else {
                    50.0
                };
                SoundDecoder::new(rate, carrier, deemphasis)
            })
            .transpose()?;
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
        self.rgb.fill(0);
        self.sync_level = 0.0;
    }

    /// Rows between one field's lines in the frame: interlaced fields land on alternate rows.
    fn row_step(&self) -> u32 {
        if self.params.interlace { 2 } else { 1 }
    }

    fn push_sample(&mut self, v: f32, out: &mut ChannelOutputs) {
        let n = self.levels.normalize(v);
        self.sync_level += self.sync_coeff * (n - self.sync_level);
        if self.sync_level < SYNC_SLICE {
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
        let carrier = color_carrier_hz(self.params.color);
        let color_phase =
            carrier.map(|hz| burst_phase(&self.line[..len], black, hz, self.video_rate));
        for k in 0..usize::from(self.width) {
            let x = start + (k as f64 + 0.5) * active / f64::from(self.width);
            let v = interpolate(&self.line[..len], x);
            let luma = if let (Some(hz), Some(_)) = (carrier, color_phase) {
                local_luma(&self.line[..len], x, black, range, hz, self.video_rate)
            } else {
                ((v - black) / range).clamp(0.0, 1.0)
            };
            self.frame[base + k] = (luma * 255.0) as u8;
            if let (Some(hz), Some(phase)) = (carrier, color_phase) {
                let (u, mut v) = local_chroma(
                    &self.line[..len],
                    x,
                    (black, range),
                    hz,
                    self.video_rate,
                    phase,
                    luma,
                );
                if self.params.color == AtvColor::Pal && self.field_row % 2 == 1 {
                    v = -v;
                }
                let rgb = yuv_to_rgb(luma, u, v);
                let rgb_base = (base + k) * 3;
                self.rgb[rgb_base..rgb_base + 3].copy_from_slice(&rgb);
            }
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
        // The one allocation on this path, and the same bounded deviation the PCM
        // hand-off takes: a picture per field is 50 a second, and the alternative is a pool the
        // host would have to hand back on a thread it does not own.
        out.video.push(VideoPicture {
            width: self.width,
            height: self.height,
            luma: self.frame.clone(),
            rgb: self.rgb.clone(),
        });
    }
}

fn mean(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f32>() / samples.len() as f32
}

fn interpolate(samples: &[f32], x: f64) -> f32 {
    let i = x as usize;
    if i + 1 < samples.len() {
        let fraction = (x - i as f64) as f32;
        samples[i] + (samples[i + 1] - samples[i]) * fraction
    } else {
        samples[samples.len() - 1]
    }
}

fn color_window(rate: f64, carrier_hz: f64) -> usize {
    (6.0 * rate / carrier_hz).round().max(8.0) as usize
}

fn sample_window(samples: &[f32], x: f64, width: usize) -> (usize, usize) {
    let center = x.round() as usize;
    let start = center.saturating_sub(width / 2);
    let end = (start + width).min(samples.len());
    (start, end)
}

fn local_luma(samples: &[f32], x: f64, black: f32, range: f32, carrier_hz: f64, rate: f64) -> f32 {
    let (start, end) = sample_window(samples, x, color_window(rate, carrier_hz));
    ((mean(&samples[start..end]) - black) / range).clamp(0.0, 1.0)
}

fn burst_phase(samples: &[f32], black: f32, carrier_hz: f64, rate: f64) -> f64 {
    let start = (5.5e-6 * rate).round() as usize;
    let end = ((8.0e-6 * rate).round() as usize).min(samples.len());
    let (mut cosine, mut sine) = (0.0, 0.0);
    let step = TAU * carrier_hz / rate;
    let (step_sin, step_cos) = step.sin_cos();
    let (mut phase_sin, mut phase_cos) = (step * start as f64).sin_cos();
    for &sample in &samples[start.min(end)..end] {
        let sample = f64::from(sample - black);
        cosine += sample * phase_cos;
        sine += sample * phase_sin;
        let next_cos = phase_cos * step_cos - phase_sin * step_sin;
        phase_sin = phase_sin * step_cos + phase_cos * step_sin;
        phase_cos = next_cos;
    }
    (-sine).atan2(cosine)
}

fn local_chroma(
    samples: &[f32],
    x: f64,
    levels: (f32, f32),
    carrier_hz: f64,
    rate: f64,
    offset: f64,
    luma: f32,
) -> (f32, f32) {
    let (black, range) = levels;
    let (start, end) = sample_window(samples, x, color_window(rate, carrier_hz));
    let (mut u, mut v, mut uc, mut vc) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let step = TAU * carrier_hz / rate;
    let (step_sin, step_cos) = step.sin_cos();
    let (mut sin, mut cos) = (step * start as f64 + offset).sin_cos();
    for &sample in &samples[start..end] {
        let centered = f64::from((sample - black) / range - luma);
        u += centered * cos;
        v += centered * sin;
        uc += cos * cos;
        vc += sin * sin;
        let next_cos = cos * step_cos - sin * step_sin;
        sin = sin * step_cos + cos * step_sin;
        cos = next_cos;
    }
    let u = if uc > 0.0 { 2.0 * u / uc } else { 0.0 } as f32;
    let v = if vc > 0.0 { 2.0 * v / vc } else { 0.0 } as f32;
    (u, v)
}

fn yuv_to_rgb(y: f32, u: f32, v: f32) -> [u8; 3] {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [
        byte(y + 1.140 * v),
        byte(y - 0.395 * u - 0.581 * v),
        byte(y + 2.033 * u),
    ]
}

impl ChannelRx for AtvChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        let (front, video_rate) = VideoFront::new(ctx.input_rate, p)?;
        let mut chan = Self {
            params: p.clone(),
            input_rate: ctx.input_rate,
            video_rate,
            timing: timing(p.standard),
            nominal_line: 0.0,
            line_len: 0.0,
            video: Vec::new(),
            filtered: Vec::new(),
            front,
            detector: Detector::new(p, video_rate),
            sound: None,
            polarity: 1.0,
            levels: Levels::new(INPUT_RATE_HZ),
            sync_level: 0.0,
            sync_coeff: 1.0,
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
            rgb: Vec::new(),
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
        if self.params.modulation == AtvModulation::Am
            && let Some(sound) = &mut self.sound
        {
            sound.process_iq(iq, out);
        }
        self.front.process(iq, &mut self.filtered);
        // Taken out so the per-sample loop can call back into `self`; put back below, so the
        // buffer's capacity survives the block and nothing here allocates in steady state.
        let mut video = std::mem::take(&mut self.video);
        self.detector.process(&self.filtered, &mut video);
        if self.params.modulation == AtvModulation::Fm
            && let Some(sound) = &mut self.sound
        {
            sound.process_composite(&video, out);
        }
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
        testgen::atv::{AtvSource, BAR_LEVELS, COLOR_BARS, bars, color_bars_with_tone},
        testutil::{dominant_tone, settings},
    };

    fn channel(p: AtvParams) -> AtvChannel {
        channel_at_rate(p, INPUT_RATE_HZ)
    }

    fn channel_at_rate(p: AtvParams, input_rate: f64) -> AtvChannel {
        AtvChannel::new(ChannelCtx { input_rate }, settings(ChannelParams::Atv(p))).unwrap()
    }

    /// Run `iq` through in ragged blocks (the sizes a device really hands over) and collect
    /// every picture that came out.
    fn run(chan: &mut AtvChannel, iq: &[Complex<f32>]) -> Vec<VideoPicture> {
        run_media(chan, iq).0
    }

    fn run_media(chan: &mut AtvChannel, iq: &[Complex<f32>]) -> (Vec<VideoPicture>, Vec<f32>) {
        let mut out = ChannelOutputs::default();
        let mut pictures = Vec::new();
        let mut audio = Vec::new();
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
            audio.extend_from_slice(&out.audio_pcm);
            at = end;
        }
        (pictures, audio)
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

    fn bar_rgb(picture: &VideoPicture, row: usize, bar: usize) -> [f64; 3] {
        let width = usize::from(picture.width);
        let lo = width * bar / COLOR_BARS.len();
        let hi = width * (bar + 1) / COLOR_BARS.len();
        let inset = (hi - lo) / 4;
        let mut sum = [0.0; 3];
        let count = hi - lo - 2 * inset;
        for x in lo + inset..hi - inset {
            let at = (row * width + x) * 3;
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += f64::from(picture.rgb[at + channel]);
            }
        }
        sum.map(|value| value / count as f64)
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

    #[test]
    fn decodes_pal_colour_and_the_am_sound_carrier() {
        const RATE: f64 = 12_000_000.0;
        let p = AtvParams {
            color: AtvColor::Pal,
            sound_subcarrier_hz: Some(5_500_000.0),
            ..params_for(AtvStandard::Ccir625, AtvModulation::Am)
        };
        let iq = color_bars_with_tone(&AtvSource::new(&p, RATE), 4);
        let (pictures, audio) = run_media(&mut channel_at_rate(p, RATE), &iq);
        let picture = pictures.last().expect("a PAL picture");
        assert_eq!(
            picture.rgb.len(),
            usize::from(picture.width) * usize::from(picture.height) * 3
        );
        let row = usize::from(picture.height) / 2;
        for (bar, expected) in COLOR_BARS.iter().enumerate() {
            let got = bar_rgb(picture, row, bar);
            for channel in 0..3 {
                let want = f64::from(expected[channel]) * 255.0;
                assert!(
                    (got[channel] - want).abs() < 75.0,
                    "bar {bar} channel {channel}: got {got:?}, expected {expected:?}"
                );
            }
        }
        let settled = &audio[audio.len() / 2..];
        let (tone, ratio) = dominant_tone(settled, f64::from(AUDIO_RATE));
        assert!((tone - 1_000.0).abs() < 20.0, "sound tone {tone} Hz");
        assert!(ratio > 20.0, "sound tone ratio {ratio}");
    }

    #[test]
    fn decodes_ntsc_colour() {
        const RATE: f64 = 12_000_000.0;
        let p = AtvParams {
            color: AtvColor::Ntsc,
            ..params_for(AtvStandard::Eia525, AtvModulation::Am)
        };
        let iq = color_bars_with_tone(&AtvSource::new(&p, RATE), 4);
        let pictures = run(&mut channel_at_rate(p, RATE), &iq);
        let picture = pictures.last().expect("an NTSC picture");
        assert!(!picture.rgb.is_empty());
        let row = usize::from(picture.height) / 2;
        let red = bar_rgb(picture, row, 5);
        let blue = bar_rgb(picture, row, 6);
        assert!(
            red[0] > red[1] + 60.0 && red[0] > red[2] + 60.0,
            "red {red:?}"
        );
        assert!(
            blue[2] > blue[0] + 60.0 && blue[2] > blue[1] + 60.0,
            "blue {blue:?}"
        );
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

    /// Native-rate colour runs a wider FIR, chroma matrix and sound DDC together. The debug-build
    /// budget catches an order-of-magnitude regression; release builds optimize the sample loop.
    #[test]
    fn colour_and_sound_path_has_bounded_cost() {
        const RATE: f64 = 12_000_000.0;
        let p = AtvParams {
            color: AtvColor::Pal,
            sound_subcarrier_hz: Some(5_500_000.0),
            ..params_for(AtvStandard::Ccir625, AtvModulation::Am)
        };
        let iq = color_bars_with_tone(&AtvSource::new(&p, RATE), 4);
        let seconds = iq.len() as f64 / RATE;
        let mut chan = channel_at_rate(p, RATE);
        let mut out = ChannelOutputs::default();
        let started = std::time::Instant::now();
        for block in iq.chunks(16_384) {
            out.reset();
            chan.process(block, &mut out);
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(
            elapsed < seconds * 10.0,
            "{seconds:.2} s of colour video took {elapsed:.2} s"
        );
    }
}
