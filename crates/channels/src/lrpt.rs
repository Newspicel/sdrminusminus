mod demod;
mod frame;
pub(crate) mod image;
pub(crate) mod jpeg;
pub(crate) mod link;
mod packet;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use demod::Demod;
use frame::{Deframer, FrameOutcome};
use image::{IDLE_APID, Imagery, Placement};
use num_complex::Complex;
use packet::Depacketizer;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DecoderFamily, LrptImage,
    LrptMode, LrptParams,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, DecodedImage,
    check_input_rate,
};
use link::PacketHeader;

pub(crate) const INPUT_RATE_HZ: f64 = 288_000.0;
const FILTER_TAPS: usize = 127;
const CHUNK: usize = 4_096;
const SIGNAL_LOSS_S: f64 = 2.0;
const MIN_COMPLETE_LINES: u16 = 16;
const SOURCE: &str = "lrpt";

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "lrpt".to_owned(),
    name: "Meteor LRPT".to_owned(),
    summary: "Meteor-M weather satellite pictures".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 140_000.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    decoder_kind: Some("lrpt".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &LrptParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &LrptParams) -> Result<ChannelFilter, ChannelError> {
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(FILTER_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&LrptParams, ChannelError> {
    match &settings.params {
        ChannelParams::Lrpt(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "lrpt channel got {} params",
            other.type_id()
        ))),
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameStats {
    frames: u32,
    corrected: u32,
    failed: u32,
}

impl FrameStats {
    fn count(&mut self, outcome: FrameOutcome) {
        self.frames = self.frames.saturating_add(1);
        if outcome.failed {
            self.failed = self.failed.saturating_add(1);
        } else if outcome.corrected > 0 {
            self.corrected = self.corrected.saturating_add(1);
        }
    }
}

struct Picture {
    imagery: Imagery,
    stats: FrameStats,
    seq: u32,
    started: u64,
    last_frame: u64,
    shown_rows: usize,
}

impl Picture {
    fn new() -> Self {
        Self {
            imagery: Imagery::new(),
            stats: FrameStats::default(),
            seq: 0,
            started: 0,
            last_frame: 0,
            shown_rows: 0,
        }
    }

    fn clear(&mut self) {
        self.imagery.reset();
        self.stats = FrameStats::default();
        self.shown_rows = 0;
    }

    fn place(&mut self, packet: &[u8], now: u64, mode: LrptMode, out: &mut ChannelOutputs) {
        let header = PacketHeader::parse(packet);
        if header.apid == IDLE_APID {
            return;
        }
        let was_empty = self.imagery.is_empty();
        if self.imagery.place(packet, header.apid, header.sequence) == Placement::Full {
            self.finish(mode, true, out);
            self.imagery.place(packet, header.apid, header.sequence);
        }
        if was_empty && !self.imagery.is_empty() {
            self.started = now;
        }
    }

    fn progress(&mut self, out: &mut ChannelOutputs) {
        if self.imagery.rows() > self.shown_rows {
            self.shown_rows = self.imagery.rows();
            out.video.push(self.imagery.snapshot());
        }
    }

    fn finish(&mut self, mode: LrptMode, complete: bool, out: &mut ChannelOutputs) {
        if self.imagery.is_empty() {
            self.clear();
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let lines = self.imagery.lines();
        let elapsed = self.last_frame.saturating_sub(self.started);
        let picture = self.imagery.snapshot();
        out.events.push(DecoderEvent::Lrpt(LrptImage {
            seq: self.seq,
            mode,
            width: picture.width,
            lines,
            complete,
            duration_ms: (elapsed as f64 * 1_000.0 / INPUT_RATE_HZ) as u32,
            apids: self.imagery.apids(),
            frames: self.stats.frames,
            frames_corrected: self.stats.corrected,
            frames_failed: self.stats.failed,
            packets_lost: self.imagery.packets_lost(),
        }));
        out.video.push(picture.clone());
        out.images.push(DecodedImage {
            source: SOURCE,
            mode: self.imagery.label(),
            complete,
            lines,
            picture,
        });
        self.clear();
    }
}

pub struct LrptChannel {
    mode: LrptMode,
    demod: Demod,
    deframer: Deframer,
    depacketizer: Depacketizer,
    picture: Picture,
    soft: Vec<i16>,
    samples: u64,
    closing: bool,
}

impl LrptChannel {
    fn restart(&mut self) {
        self.demod.reset();
        self.deframer.reset();
        self.depacketizer.reset();
        self.closing = true;
    }

    fn on_frame(&mut self, outcome: FrameOutcome, out: &mut ChannelOutputs) {
        let picture = &mut self.picture;
        picture.stats.count(outcome);
        if outcome.failed {
            self.depacketizer.reset();
            return;
        }
        picture.last_frame = self.samples;
        self.depacketizer.load(self.deframer.vcdu());
        while let Some(packet) = self.depacketizer.next_packet() {
            picture.place(packet, self.samples, self.mode, out);
        }
    }

    fn check_signal(&mut self, out: &mut ChannelOutputs) {
        let silent = self.samples.saturating_sub(self.picture.last_frame);
        if (silent as f64) < SIGNAL_LOSS_S * INPUT_RATE_HZ {
            return;
        }
        let complete = self.picture.imagery.lines() >= MIN_COMPLETE_LINES;
        self.picture.finish(self.mode, complete, out);
        self.picture.last_frame = self.samples;
    }

    fn run_chunk(&mut self, chunk: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.soft.clear();
        self.demod.process(chunk, &mut self.soft);
        self.samples += chunk.len() as u64;
        self.deframer.push(&self.soft);
        while let Some(outcome) = self.deframer.next_frame() {
            self.on_frame(outcome, out);
        }
        self.picture.progress(out);
        self.check_signal(out);
    }
}

impl ChannelRx for LrptChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let mode = params(&settings)?.mode;
        Ok(Self {
            mode,
            demod: Demod::new(mode, INPUT_RATE_HZ),
            deframer: Deframer::new(),
            depacketizer: Depacketizer::new(),
            picture: Picture::new(),
            soft: Vec::with_capacity(2 * CHUNK),
            samples: 0,
            closing: false,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let mode = params(&settings)?.mode;
        if mode != self.mode {
            self.mode = mode;
            self.demod = Demod::new(mode, INPUT_RATE_HZ);
            self.restart();
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.restart();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        if self.closing {
            self.closing = false;
            self.picture.finish(self.mode, false, out);
        }
        for chunk in iq.chunks(CHUNK) {
            self.run_chunk(chunk, out);
        }
    }
}
