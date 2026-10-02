mod envelope;
pub(crate) mod geometry;
mod picture;
mod sync;
mod telemetry;
mod track;

use std::sync::LazyLock;

use envelope::{CHUNK, Envelope, track_rate};
use geometry::LINE_SECONDS;
use num_complex::Complex;
use picture::Picture;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    AptImage, AptParams, ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent,
    DecoderFamily,
};
use sync::{Acquisition, LineClock, SyncDetector};
use track::Track;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, DecodedImage,
    check_input_rate,
};

pub(crate) const INPUT_RATE_HZ: f64 = 60_000.0;
const FILTER_TAPS: usize = 127;
const LOST_SYNC_LIMIT: u16 = 16;
const MIN_KEPT_LINES: usize = 16;
const MIN_COMPLETE_LINES: usize = 120;

pub(crate) const SOURCE: &str = "apt";

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "apt".to_owned(),
    name: "NOAA APT".to_owned(),
    summary: "NOAA weather satellite pictures".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 40_000.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    decoder_kind: Some("apt".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &AptParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &AptParams) -> Result<ChannelFilter, ChannelError> {
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(FILTER_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&AptParams, ChannelError> {
    match &settings.params {
        ChannelParams::Apt(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "apt channel got {} params",
            other.type_id()
        ))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Faded,
    Capped,
    Aborted,
}

pub struct AptChannel {
    envelope: Envelope,
    track: Track,
    detector: SyncDetector,
    acquisition: Acquisition,
    clock: LineClock,
    picture: Picture,
    rate: f64,
    line: f64,
    keep_partial: bool,
    abandoned: bool,
    seq: u32,
}

impl AptChannel {
    fn advance(&mut self, out: &mut ChannelOutputs) {
        while self.acquire() || self.decode_line(out) {}
    }

    fn acquire(&mut self) -> bool {
        if self.picture.active {
            return false;
        }
        if self.track.aged_out(self.acquisition.from) {
            self.acquisition.restart(self.track.oldest());
        }
        let from = self.acquisition.from;
        let to = from + self.line as u64;
        if !self.track.buffered(from, to + self.detector.len() + 1) {
            return false;
        }
        let peak = self.detector.best(&self.track, from, to);
        self.acquisition.from = to;
        if let Some((first, len)) =
            self.acquisition
                .observe(peak, self.line, self.detector.tolerance())
        {
            self.clock.lock(first, len);
            self.picture.begin(first);
        }
        true
    }

    fn decode_line(&mut self, out: &mut ChannelOutputs) -> bool {
        if !self.picture.active {
            return false;
        }
        let search = self.detector.search();
        let from = (self.clock.start - search).max(0.0) as u64;
        let to = (self.clock.start + search).ceil() as u64;
        let needed = (self.clock.start + self.clock.len).ceil() as u64
            + search.ceil() as u64
            + self.detector.len()
            + 2;
        if self.track.aged_out(from) {
            self.finish(Ending::Aborted, out);
            return true;
        }
        if !self.track.buffered(from, needed) {
            return false;
        }
        let peak = self.detector.best(&self.track, from, to);
        let synced = self.clock.observe(peak);
        self.picture
            .store_line(&self.track, self.clock.start, self.clock.len, synced);
        self.clock.next_line();
        if self.picture.progress_due() {
            out.video.push(self.picture.snapshot(self.picture.lines));
        }
        if self.picture.full() {
            self.finish(Ending::Capped, out);
            self.picture.begin(self.clock.start);
        } else if self.clock.lost >= LOST_SYNC_LIMIT {
            self.finish(Ending::Faded, out);
        }
        true
    }

    fn finish(&mut self, ending: Ending, out: &mut ChannelOutputs) {
        if !self.picture.active {
            return;
        }
        self.picture.active = false;
        self.acquisition.restart(self.clock.start.max(0.0) as u64);
        self.emit(ending, out);
    }

    fn emit(&mut self, ending: Ending, out: &mut ChannelOutputs) {
        let lines = match ending {
            Ending::Capped => self.picture.lines,
            Ending::Faded | Ending::Aborted => self.picture.synced_lines,
        };
        let complete = ending == Ending::Faded && lines >= MIN_COMPLETE_LINES;
        let kept =
            complete || ending == Ending::Capped || (self.keep_partial && lines >= MIN_KEPT_LINES);
        if !kept {
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let duration_ms = (lines as f64 * self.clock.len / self.rate * 1_000.0) as u32;
        let picture = self.picture.snapshot(lines);
        out.events.push(DecoderEvent::Apt(AptImage {
            seq: self.seq,
            lines: picture.height,
            complete,
            duration_ms,
            channel_a: self.picture.channel_a,
            channel_b: self.picture.channel_b,
        }));
        out.video.push(picture.clone());
        out.images.push(DecodedImage {
            source: SOURCE,
            mode: self.picture.mode(),
            complete,
            lines: picture.height,
            picture,
        });
    }

    fn flush_abandoned(&mut self, out: &mut ChannelOutputs) {
        if !self.abandoned {
            return;
        }
        self.abandoned = false;
        self.emit(Ending::Aborted, out);
    }
}

impl ChannelRx for AptChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        let rate = track_rate(ctx.input_rate);
        let line = LINE_SECONDS * rate;
        Ok(Self {
            envelope: Envelope::new(ctx.input_rate),
            track: Track::new(),
            detector: SyncDetector::new(rate),
            acquisition: Acquisition::default(),
            clock: LineClock::new(line),
            picture: Picture::new(),
            rate,
            line,
            keep_partial: p.keep_partial,
            abandoned: false,
            seq: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.keep_partial = params(&settings)?.keep_partial;
        Ok(())
    }

    fn retuned(&mut self) {
        if self.picture.active {
            self.picture.active = false;
            self.abandoned = true;
        }
        self.envelope.reset();
        self.acquisition.restart(self.track.head());
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.flush_abandoned(out);
        for chunk in iq.chunks(CHUNK) {
            self.envelope.process(chunk, &mut self.track);
            self.advance(out);
        }
    }
}

#[cfg(test)]
mod tests;
