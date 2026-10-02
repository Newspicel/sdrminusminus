mod phasing;
mod picture;
#[cfg(test)]
mod tests;
mod tones;
mod track;

use std::sync::LazyLock;

use num_complex::Complex;
use phasing::{Lock, Phasing, Scan};
use picture::Picture;
use sdrmm_dsp::{FirC, FmDemod, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DecoderFamily, WefaxIoc,
    WefaxParams, WefaxPicture,
};
use tones::{Signal, ToneDetector, ToneKind};
use track::Track;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, DecodedImage,
    check_input_rate,
};

pub(crate) const INPUT_RATE_HZ: f64 = 12_000.0;
const FILTER_TAPS: usize = 127;

pub(crate) const BLACK_HZ: f64 = 1_500.0;
pub(crate) const WHITE_HZ: f64 = 2_300.0;
pub(crate) const STOP_TONE_HZ: f64 = 450.0;
pub(crate) const PULSE_FRACTION: f64 = 0.05;
pub(crate) const SOURCE: &str = "wefax";

const WRITE_CHUNK: usize = 1 << 13;
const FALLBACK_LINES: f64 = 3.0;
const PROGRESS_LINES: u16 = 8;
const MIN_KEPT_LINES: u16 = 8;
const BOUNDARY_SLACK_BLOCKS: f64 = 1.5;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "wefax".to_owned(),
    name: "WEFAX".to_owned(),
    summary: "HF weather fax charts".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 1_600.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    decoder_kind: Some("wefax".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &WefaxParams) -> (f64, f64) {
    (1_100.0, 2_700.0)
}

pub(crate) fn channel_filter(p: &WefaxParams) -> Result<ChannelFilter, ChannelError> {
    let (low, high) = occupied_band(p);
    let half = (high - low) / 2.0 / INPUT_RATE_HZ;
    let center = (high + low) / 2.0 / INPUT_RATE_HZ;
    Ok(ChannelFilter::Sideband(FirC::from_lowpass(
        &design_lowpass(FILTER_TAPS, half),
        center,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&WefaxParams, ChannelError> {
    match &settings.params {
        ChannelParams::Wefax(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "wefax channel got {} params",
            other.type_id()
        ))),
    }
}

#[cfg(any(test, feature = "synth"))]
#[must_use]
pub(crate) fn level_to_hz(level: u8) -> f64 {
    BLACK_HZ + (WHITE_HZ - BLACK_HZ) * f64::from(level) / 255.0
}

fn hz_to_unit(freq: f32) -> f32 {
    ((f64::from(freq) - BLACK_HZ) / (WHITE_HZ - BLACK_HZ)).clamp(0.0, 1.0) as f32
}

fn samples(ms: f64, rate: f64) -> f64 {
    ms * rate / 1_000.0
}

pub struct WefaxChannel {
    demod: FmDemod,
    freq: Vec<f32>,
    track: Track,
    tones: ToneDetector,
    phasing: Phasing,
    picture: Picture,
    rate: f64,
    params: WefaxParams,
    heard_ioc: Option<WefaxIoc>,
    armed_at: Option<u64>,
    began: Option<u64>,
    seq: u32,
}

impl WefaxChannel {
    fn line_period(&self) -> f64 {
        samples(self.params.lpm.line_ms(), self.rate)
    }

    fn feed(&mut self, sample: Complex<f32>, freq: f32, out: &mut ChannelOutputs) {
        let index = self.track.head();
        self.track.push(hz_to_unit(freq));
        if let Some(signal) = self.tones.push(index, freq, sample.norm()) {
            self.on_tone(signal, out);
        }
    }

    fn on_tone(&mut self, signal: Signal, out: &mut ChannelOutputs) {
        match signal {
            Signal::Confirmed(ToneKind::Start(ioc), at) => {
                self.close(at, false, out);
                self.forget();
                self.heard_ioc = Some(ioc);
                self.began = Some(at);
            }
            Signal::Ended(ToneKind::Start(_), at) if !self.picture.active => {
                self.armed_at = Some(at);
                self.phasing.reset(at);
            }
            Signal::Confirmed(ToneKind::Stop, at) => {
                self.close(at, true, out);
                self.forget();
            }
            Signal::Ended(..) => {}
        }
    }

    fn forget(&mut self) {
        self.heard_ioc = None;
        self.armed_at = None;
        self.began = None;
        self.phasing.reset(self.track.head());
    }

    fn advance(&mut self, out: &mut ChannelOutputs) {
        loop {
            let progressed = if self.picture.active {
                self.step_picture(out)
            } else {
                self.step_idle()
            };
            if !progressed {
                return;
            }
        }
    }

    fn step_idle(&mut self) -> bool {
        let period = self.line_period();
        if self.tones.outage().is_some() {
            self.armed_at = None;
        }
        if let Some(armed) = self.armed_at
            && self
                .phasing
                .idle_past(armed + (period * FALLBACK_LINES) as u64)
        {
            self.begin(Lock {
                origin: armed as f64,
                period,
                first: armed,
            });
            return true;
        }
        match self.phasing.scan(&self.track, period) {
            Scan::Waiting => false,
            Scan::Searched => true,
            Scan::Locked(lock) => {
                self.begin(lock);
                true
            }
        }
    }

    fn begin(&mut self, lock: Lock) {
        let ioc = self.heard_ioc.take().unwrap_or(self.params.ioc);
        self.picture
            .begin(ioc, self.params.lpm, lock.origin, lock.period);
        self.picture.started = self.began.take().unwrap_or(lock.first);
        self.armed_at = None;
    }

    fn step_picture(&mut self, out: &mut ChannelOutputs) -> bool {
        if let Some(at) = self.tones.outage() {
            self.close(at, false, out);
            self.forget();
            return true;
        }
        if self.picture.full() {
            self.finish(false, self.track.head(), out);
            self.forget();
            return true;
        }
        let from = self.picture.line_start(self.picture.line).max(0.0) as u64;
        let to = self.picture.line_end(self.picture.line).ceil() as u64;
        if self.track.aged_out(from) {
            self.finish(false, self.track.head(), out);
            self.forget();
            return true;
        }
        if !self.track.buffered(from, to) {
            return false;
        }
        self.picture.scan_line(&self.track);
        self.emit_progress(out);
        true
    }

    fn close(&mut self, boundary: u64, complete: bool, out: &mut ChannelOutputs) {
        if !self.picture.active {
            return;
        }
        let limit = boundary as f64 + self.tones.block_len() as f64 * BOUNDARY_SLACK_BLOCKS;
        while !self.picture.full() && self.picture.line_end(self.picture.line) <= limit {
            let from = self.picture.line_start(self.picture.line).max(0.0) as u64;
            let to = self.picture.line_end(self.picture.line).ceil() as u64;
            if !self.track.buffered(from, to) {
                break;
            }
            self.picture.scan_line(&self.track);
        }
        self.picture.truncate(limit);
        self.finish(complete, boundary, out);
    }

    fn emit_progress(&mut self, out: &mut ChannelOutputs) {
        if self.picture.since_progress < PROGRESS_LINES {
            return;
        }
        self.picture.since_progress = 0;
        out.video.push(self.picture.snapshot());
    }

    fn finish(&mut self, complete: bool, end: u64, out: &mut ChannelOutputs) {
        if !self.picture.active {
            return;
        }
        self.picture.active = false;
        let lines = self.picture.decoded;
        if lines == 0 || (!complete && (!self.params.keep_partial || lines < MIN_KEPT_LINES)) {
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let elapsed = end.saturating_sub(self.picture.started);
        let picture = self.picture.snapshot();
        out.events.push(DecoderEvent::Wefax(WefaxPicture {
            seq: self.seq,
            ioc: self.picture.ioc,
            lpm: self.picture.lpm,
            width: self.picture.width(),
            lines,
            complete,
            duration_ms: (elapsed as f64 * 1_000.0 / self.rate) as u32,
        }));
        out.video.push(picture.clone());
        out.images.push(DecodedImage {
            source: SOURCE,
            mode: self.picture.label(),
            complete,
            lines,
            picture,
        });
    }

    fn drop_all(&mut self) {
        self.picture.active = false;
        self.tones.reset(self.track.head());
        self.forget();
    }
}

impl ChannelRx for WefaxChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, 1.0),
            freq: Vec::with_capacity(WRITE_CHUNK),
            track: Track::new(),
            tones: ToneDetector::new(ctx.input_rate),
            phasing: Phasing::new(),
            picture: Picture::empty(),
            rate: ctx.input_rate,
            params: *params(&settings)?,
            heard_ioc: None,
            armed_at: None,
            began: None,
            seq: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let next = *params(&settings)?;
        if next.lpm != self.params.lpm {
            self.drop_all();
        }
        self.params = next;
        Ok(())
    }

    fn retuned(&mut self) {
        self.drop_all();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.freq);
        let freq = std::mem::take(&mut self.freq);
        for (samples, freqs) in iq.chunks(WRITE_CHUNK).zip(freq.chunks(WRITE_CHUNK)) {
            for (&sample, &hz) in samples.iter().zip(freqs) {
                self.feed(sample, hz, out);
            }
            self.advance(out);
        }
        self.freq = freq;
    }
}
