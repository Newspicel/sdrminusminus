pub(crate) mod modes;

use std::sync::LazyLock;

use modes::{
    LEADER_HZ, Part, SYNC_HZ, Scan, VIS_BIT_MS, VIS_ONE_HZ, VIS_ZERO_HZ, hz_to_level, timing,
};
use num_complex::Complex;
use sdrmm_dsp::{FirC, FmDemod, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, SstvMode, SstvParams,
    SstvPicture,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, DecodedImage, VideoPicture,
    check_input_rate,
};

pub(crate) const INPUT_RATE_HZ: f64 = 16_000.0;
const AUDIO_LOW_HZ: f64 = 1_000.0;
const AUDIO_HIGH_HZ: f64 = 2_600.0;
const FILTER_TAPS: usize = 255;

const TRACK_CAPACITY: usize = 1 << 16;
const WRITE_CHUNK: usize = TRACK_CAPACITY / 4;

const MAX_WIDTH: usize = 640;
const MAX_HEIGHT: usize = 496;
const MAX_PIXELS: usize = MAX_WIDTH * MAX_HEIGHT;

const TONE_TOLERANCE_HZ: f32 = 90.0;
const VIS_TOLERANCE_HZ: f32 = 180.0;
const SYNC_TOLERANCE_HZ: f32 = 150.0;
const GLITCH_MS: f64 = 1.5;
const LEADER_MIN_MS: f64 = 200.0;
const BREAK_MIN_MS: f64 = 4.0;
const BREAK_MAX_MS: f64 = 25.0;

const SEARCH_MS: f64 = 6.0;
const SYNC_MATCH: f32 = 0.6;
const SLANT_DAMPING: f64 = 0.5;
const LOST_SYNC_LIMIT: u16 = 24;
const PROGRESS_LINES: u16 = 8;
const MIN_KEPT_LINES: u16 = 8;
const NEUTRAL_CHROMA: u8 = 128;

pub(crate) const SOURCE: &str = "sstv";

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "sstv".to_owned(),
    name: "SSTV".to_owned(),
    bandwidth_hz: AUDIO_HIGH_HZ - AUDIO_LOW_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    decoder_kind: Some("sstv".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &SstvParams) -> (f64, f64) {
    (AUDIO_LOW_HZ, AUDIO_HIGH_HZ)
}

pub(crate) fn channel_filter(_p: &SstvParams) -> Result<ChannelFilter, ChannelError> {
    let half = (AUDIO_HIGH_HZ - AUDIO_LOW_HZ) / 2.0 / INPUT_RATE_HZ;
    let center = (AUDIO_HIGH_HZ + AUDIO_LOW_HZ) / 2.0 / INPUT_RATE_HZ;
    Ok(ChannelFilter::Sideband(FirC::from_lowpass(
        &design_lowpass(FILTER_TAPS, half),
        center,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&SstvParams, ChannelError> {
    match &settings.params {
        ChannelParams::Sstv(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "sstv channel got {} params",
            other.type_id()
        ))),
    }
}

fn samples(ms: f64, rate: f64) -> f64 {
    ms * rate / 1_000.0
}

struct Track {
    buf: Vec<f32>,
    head: u64,
}

impl Track {
    fn new() -> Self {
        Self {
            buf: vec![0.0; TRACK_CAPACITY],
            head: 0,
        }
    }

    fn push(&mut self, value: f32) {
        self.buf[(self.head as usize) & (TRACK_CAPACITY - 1)] = value;
        self.head += 1;
    }

    fn oldest(&self) -> u64 {
        self.head.saturating_sub(TRACK_CAPACITY as u64)
    }

    fn get(&self, index: u64) -> f32 {
        if index >= self.head || index < self.oldest() {
            return 0.0;
        }
        self.buf[(index as usize) & (TRACK_CAPACITY - 1)]
    }

    fn buffered(&self, from: u64, to: u64) -> bool {
        from >= self.oldest() && to <= self.head
    }

    fn aged_out(&self, from: u64) -> bool {
        from < self.oldest()
    }

    fn mean(&self, from: u64, to: u64) -> f32 {
        let to = to.max(from + 1);
        let mut sum = 0.0f64;
        for index in from..to {
            sum += f64::from(self.get(index));
        }
        (sum / (to - from) as f64) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Leader,
    Break,
    Other,
}

fn is_sync(freq: f32) -> bool {
    (freq - SYNC_HZ as f32).abs() < SYNC_TOLERANCE_HZ
}

fn classify(freq: f32) -> Tone {
    if (freq - LEADER_HZ as f32).abs() < TONE_TOLERANCE_HZ {
        Tone::Leader
    } else if (freq - SYNC_HZ as f32).abs() < TONE_TOLERANCE_HZ {
        Tone::Break
    } else {
        Tone::Other
    }
}

#[derive(Clone, Copy)]
struct Run {
    tone: Tone,
    start: u64,
    len: u64,
}

const BLANK_RUN: Run = Run {
    tone: Tone::Other,
    start: 0,
    len: 0,
};

struct RunTracker {
    glitch: u64,
    leader_min: u64,
    break_min: u64,
    break_max: u64,
    current: Run,
    pending: Tone,
    mismatch: u64,
    previous: [Run; 2],
}

impl RunTracker {
    fn new(rate: f64) -> Self {
        let span = |ms: f64| samples(ms, rate) as u64;
        Self {
            glitch: span(GLITCH_MS).max(1),
            leader_min: span(LEADER_MIN_MS),
            break_min: span(BREAK_MIN_MS),
            break_max: span(BREAK_MAX_MS),
            current: BLANK_RUN,
            pending: Tone::Other,
            mismatch: 0,
            previous: [BLANK_RUN; 2],
        }
    }

    fn reset(&mut self) {
        self.current = BLANK_RUN;
        self.previous = [BLANK_RUN; 2];
        self.mismatch = 0;
        self.pending = Tone::Other;
    }

    fn push(&mut self, index: u64, freq: f32) -> Option<u64> {
        let tone = classify(freq);
        if tone == self.current.tone {
            self.current.len += 1 + self.mismatch;
            self.mismatch = 0;
            return None;
        }
        if tone != self.pending {
            self.pending = tone;
            self.mismatch = 1;
            return None;
        }
        self.mismatch += 1;
        if self.mismatch < self.glitch {
            return None;
        }
        let closed = self.current;
        let detected = self.vis_start(closed, tone);
        self.previous = [self.previous[1], closed];
        self.current = Run {
            tone,
            start: index + 1 - self.mismatch,
            len: self.mismatch,
        };
        self.mismatch = 0;
        detected
    }

    fn vis_start(&self, closed: Run, opening: Tone) -> Option<u64> {
        let [first, second] = self.previous;
        let header = opening == Tone::Break
            && closed.tone == Tone::Leader
            && closed.len >= self.leader_min
            && second.tone == Tone::Break
            && (self.break_min..=self.break_max).contains(&second.len)
            && first.tone == Tone::Leader
            && first.len >= self.leader_min;
        header.then_some(closed.start + closed.len)
    }
}

struct Picture {
    active: bool,
    timing: &'static modes::Timing,
    width: u16,
    height: u16,
    origin: f64,
    line: u16,
    started: u64,
    lost_syncs: u16,
    decoded_lines: u16,
    since_progress: u16,
    rgb: Vec<u8>,
    luma: Vec<u8>,
    red: Vec<u8>,
    green: Vec<u8>,
    blue: Vec<u8>,
    line_luma: [Vec<u8>; 2],
    line_cr: Vec<u8>,
    line_cb: Vec<u8>,
    held_cr: Vec<u8>,
    held_cb: Vec<u8>,
}

impl Picture {
    fn empty() -> Self {
        let timing = timing(SstvMode::MartinM1);
        Self {
            active: false,
            timing,
            width: 0,
            height: 0,
            origin: 0.0,
            line: 0,
            started: 0,
            lost_syncs: 0,
            decoded_lines: 0,
            since_progress: 0,
            rgb: vec![0; MAX_PIXELS * 3],
            luma: vec![0; MAX_PIXELS],
            red: vec![0; MAX_WIDTH],
            green: vec![0; MAX_WIDTH],
            blue: vec![0; MAX_WIDTH],
            line_luma: [vec![0; MAX_WIDTH], vec![0; MAX_WIDTH]],
            line_cr: vec![NEUTRAL_CHROMA; MAX_WIDTH],
            line_cb: vec![NEUTRAL_CHROMA; MAX_WIDTH],
            held_cr: vec![NEUTRAL_CHROMA; MAX_WIDTH],
            held_cb: vec![NEUTRAL_CHROMA; MAX_WIDTH],
        }
    }

    fn begin(&mut self, mode: SstvMode, origin: f64, started: u64) {
        self.timing = timing(mode);
        let (width, height) = self.timing.size();
        self.active = true;
        self.width = width;
        self.height = height;
        self.origin = origin;
        self.line = 0;
        self.started = started;
        self.lost_syncs = 0;
        self.decoded_lines = 0;
        self.since_progress = 0;
        let pixels = usize::from(width) * usize::from(height);
        self.rgb[..pixels * 3].fill(0);
        self.luma[..pixels].fill(0);
        self.line_cr.fill(NEUTRAL_CHROMA);
        self.line_cb.fill(NEUTRAL_CHROMA);
        self.held_cr.fill(NEUTRAL_CHROMA);
        self.held_cb.fill(NEUTRAL_CHROMA);
    }

    fn done(&self) -> bool {
        self.line * self.timing.rows_per_line >= self.height
    }

    fn snapshot(&self) -> VideoPicture {
        let pixels = usize::from(self.width) * usize::from(self.height);
        VideoPicture {
            width: self.width,
            height: self.height,
            luma: self.luma[..pixels].to_vec(),
            rgb: self.rgb[..pixels * 3].to_vec(),
        }
    }

    fn write_row(&mut self, row: u16) {
        if row >= self.height {
            return;
        }
        let width = usize::from(self.width);
        let base = usize::from(row) * width;
        for x in 0..width {
            let pixel = (base + x) * 3;
            self.rgb[pixel] = self.red[x];
            self.rgb[pixel + 1] = self.green[x];
            self.rgb[pixel + 2] = self.blue[x];
            self.luma[base + x] = luma_of(self.red[x], self.green[x], self.blue[x]);
        }
    }
}

fn clamp8(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

fn luma_of(red: u8, green: u8, blue: u8) -> u8 {
    clamp8(0.299 * f32::from(red) + 0.587 * f32::from(green) + 0.114 * f32::from(blue))
}

fn ycrcb_to_rgb(y: u8, cr: u8, cb: u8) -> [u8; 3] {
    let y = f32::from(y);
    let cr = f32::from(cr) - 128.0;
    let cb = f32::from(cb) - 128.0;
    [
        clamp8(y + 1.402 * cr),
        clamp8(y - 0.344_136 * cb - 0.714_136 * cr),
        clamp8(y + 1.772 * cb),
    ]
}

pub struct SstvChannel {
    demod: FmDemod,
    freq: Vec<f32>,
    track: Track,
    runs: RunTracker,
    rate: f64,
    forced: Option<SstvMode>,
    slant: bool,
    keep_partial: bool,
    pending_vis: Option<u64>,
    picture: Picture,
    scores: Vec<u32>,
    seq: u32,
}

impl SstvChannel {
    fn feed(&mut self, freq: f32) {
        let index = self.track.head;
        self.track.push(freq);
        if let Some(start) = self.runs.push(index, freq) {
            self.pending_vis = Some(start);
        }
    }

    fn advance(&mut self, out: &mut ChannelOutputs) {
        while self.start_picture(out) || self.decode_line(out) {}
    }

    fn start_picture(&mut self, out: &mut ChannelOutputs) -> bool {
        let Some(start) = self.pending_vis else {
            return false;
        };
        let header = samples(VIS_BIT_MS * 10.0, self.rate).ceil() as u64;
        if self.track.aged_out(start) {
            self.pending_vis = None;
            return true;
        }
        if !self.track.buffered(start, start + header) {
            return false;
        }
        self.pending_vis = None;
        let mode = match (self.forced, self.read_vis(start)) {
            (Some(forced), _) => forced,
            (None, Some(detected)) => detected,
            (None, None) => return true,
        };
        self.finish(false, out);
        let lead_in = samples(timing(mode).lead_in_ms, self.rate);
        let origin = (start + header) as f64 + lead_in;
        self.picture.begin(mode, origin, self.track.head);
        true
    }

    fn read_vis(&self, start: u64) -> Option<SstvMode> {
        let slot = samples(VIS_BIT_MS, self.rate);
        let guard = slot * 0.2;
        let mut levels = [0.0f32; 10];
        for (index, level) in levels.iter_mut().enumerate() {
            let from = start as f64 + slot * index as f64 + guard;
            let to = start as f64 + slot * (index as f64 + 1.0) - guard;
            *level = self.track.mean(from as u64, to as u64);
        }
        let framing = |value: f32| (value - SYNC_HZ as f32).abs() < VIS_TOLERANCE_HZ;
        if !framing(levels[0]) || !framing(levels[9]) {
            return None;
        }
        let mut bits = [false; 8];
        for (bit, level) in bits.iter_mut().zip(levels[1..9].iter()) {
            let one = (level - VIS_ONE_HZ as f32).abs();
            let zero = (level - VIS_ZERO_HZ as f32).abs();
            if one.min(zero) > VIS_TOLERANCE_HZ {
                return None;
            }
            *bit = one < zero;
        }
        let code = bits[..7]
            .iter()
            .enumerate()
            .fold(0u8, |acc, (index, &bit)| acc | (u8::from(bit) << index));
        let odd = bits[..7].iter().filter(|&&bit| bit).count() % 2 == 1;
        if odd != bits[7] {
            return None;
        }
        SstvMode::from_vis(code)
    }

    fn decode_line(&mut self, out: &mut ChannelOutputs) -> bool {
        if !self.picture.active {
            return false;
        }
        let search = samples(SEARCH_MS, self.rate).ceil() as u64;
        let span = samples(self.picture.timing.line_ms, self.rate);
        let from = (self.picture.origin as u64).saturating_sub(search);
        let to = (self.picture.origin + span).ceil() as u64 + search * 2;
        if self.track.aged_out(from) {
            self.finish(false, out);
            return true;
        }
        if !self.track.buffered(from, to) {
            return false;
        }
        self.align_line(search);
        self.scan_line();
        self.emit_progress(out);
        self.picture.origin += span;
        self.picture.line += 1;
        if self.picture.done() {
            self.finish(true, out);
        } else if self.picture.lost_syncs >= LOST_SYNC_LIMIT {
            self.finish(false, out);
        }
        true
    }

    fn align_line(&mut self, search: u64) {
        let sync_len = samples(self.picture.timing.sync_ms, self.rate)
            .round()
            .max(1.0) as u64;
        let expected = self.picture.origin + samples(self.picture.timing.sync_offset_ms, self.rate);
        let first = (expected as u64).saturating_sub(search);
        self.scores.clear();
        self.scores.push(0);
        for step in 0..(search * 2 + sync_len) {
            let hit = u32::from(is_sync(self.track.get(first + step)));
            let running = self.scores[self.scores.len() - 1] + hit;
            self.scores.push(running);
        }
        let mut best = 0u32;
        let mut best_at = search;
        for offset in 0..=search * 2 {
            let index = offset as usize;
            let score = self.scores[index + sync_len as usize] - self.scores[index];
            let closer = offset.abs_diff(search) < best_at.abs_diff(search);
            if score > best || (score == best && closer) {
                best = score;
                best_at = offset;
            }
        }
        if (best as f32) < SYNC_MATCH * sync_len as f32 {
            self.picture.lost_syncs += 1;
            return;
        }
        self.picture.lost_syncs = 0;
        if self.slant {
            let shift = first as f64 + best_at as f64 - expected;
            self.picture.origin += shift * SLANT_DAMPING;
        }
    }

    fn scan_line(&mut self) {
        let origin = self.picture.origin;
        let width = usize::from(self.picture.width);
        let line = self.picture.line;
        let mut at = 0.0f64;
        for index in 0..self.picture.timing.segments.len() {
            let segment = self.picture.timing.segments[index];
            if let Part::Scan(scan) = segment.part {
                let start = origin + samples(at, self.rate);
                let length = samples(segment.ms, self.rate);
                self.sample_scan(scan, line, start, length, width);
            }
            at += segment.ms;
        }
        self.compose_line();
    }

    fn sample_scan(&mut self, scan: Scan, line: u16, start: f64, length: f64, width: usize) {
        let mut pixels = [0u8; MAX_WIDTH];
        for (index, pixel) in pixels.iter_mut().enumerate().take(width) {
            let from = start + length * index as f64 / width as f64;
            let to = start + length * (index + 1) as f64 / width as f64;
            *pixel = hz_to_level(self.track.mean(from as u64, to as u64));
        }
        let target = match scan {
            Scan::Red => &mut self.picture.red,
            Scan::Green => &mut self.picture.green,
            Scan::Blue => &mut self.picture.blue,
            Scan::Luma(0) => &mut self.picture.line_luma[0],
            Scan::Luma(_) => &mut self.picture.line_luma[1],
            Scan::ChromaR => &mut self.picture.line_cr,
            Scan::ChromaB => &mut self.picture.line_cb,
            Scan::ChromaAlternating if line.is_multiple_of(2) => &mut self.picture.line_cr,
            Scan::ChromaAlternating => &mut self.picture.line_cb,
        };
        target[..width].copy_from_slice(&pixels[..width]);
    }

    fn compose_line(&mut self) {
        let picture = &mut self.picture;
        let width = usize::from(picture.width);
        let rows = picture.timing.rows_per_line;
        let line = picture.line;
        if picture.timing.alternates_chroma() {
            if line.is_multiple_of(2) {
                picture.held_cr[..width].copy_from_slice(&picture.line_cr[..width]);
            } else {
                picture.held_cb[..width].copy_from_slice(&picture.line_cb[..width]);
            }
            picture.line_cr[..width].copy_from_slice(&picture.held_cr[..width]);
            picture.line_cb[..width].copy_from_slice(&picture.held_cb[..width]);
        }
        for row in 0..rows {
            if picture.timing.carries_luma() {
                let luma = &picture.line_luma[usize::from(row).min(1)];
                let rows = luma[..width]
                    .iter()
                    .zip(&picture.line_cr[..width])
                    .zip(&picture.line_cb[..width]);
                for (x, ((&y, &cr), &cb)) in rows.enumerate() {
                    let [r, g, b] = ycrcb_to_rgb(y, cr, cb);
                    picture.red[x] = r;
                    picture.green[x] = g;
                    picture.blue[x] = b;
                }
            }
            picture.write_row(line * rows + row);
        }
        picture.decoded_lines = ((line + 1) * rows).min(picture.height);
        picture.since_progress += 1;
    }

    fn emit_progress(&mut self, out: &mut ChannelOutputs) {
        if self.picture.since_progress < PROGRESS_LINES {
            return;
        }
        self.picture.since_progress = 0;
        out.video.push(self.picture.snapshot());
    }

    fn finish(&mut self, complete: bool, out: &mut ChannelOutputs) {
        if !self.picture.active {
            return;
        }
        self.picture.active = false;
        let lines = self.picture.decoded_lines;
        if !complete && (!self.keep_partial || lines < MIN_KEPT_LINES) {
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let elapsed = self.track.head.saturating_sub(self.picture.started);
        let duration_ms = (elapsed as f64 * 1_000.0 / self.rate) as u32;
        let picture = self.picture.snapshot();
        out.events.push(DecoderEvent::Sstv(SstvPicture {
            seq: self.seq,
            mode: self.picture.timing.mode,
            width: self.picture.width,
            height: self.picture.height,
            lines,
            complete,
            duration_ms,
        }));
        out.video.push(picture.clone());
        out.images.push(DecodedImage {
            source: SOURCE,
            mode: self.picture.timing.mode.label().to_owned(),
            complete,
            lines,
            picture,
        });
    }
}

impl ChannelRx for SstvChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let p = params(&settings)?;
        Ok(Self {
            demod: FmDemod::new(ctx.input_rate, 1.0),
            freq: Vec::new(),
            track: Track::new(),
            runs: RunTracker::new(ctx.input_rate),
            rate: ctx.input_rate,
            forced: p.mode,
            slant: p.slant_correction,
            keep_partial: p.keep_partial,
            pending_vis: None,
            picture: Picture::empty(),
            scores: Vec::with_capacity(2_048),
            seq: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let p = params(&settings)?;
        if p.mode != self.forced {
            self.picture.active = false;
            self.pending_vis = None;
        }
        self.forced = p.mode;
        self.slant = p.slant_correction;
        self.keep_partial = p.keep_partial;
        Ok(())
    }

    fn retuned(&mut self) {
        self.picture.active = false;
        self.pending_vis = None;
        self.runs.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.demod.process(iq, &mut self.freq);
        let mut at = 0;
        while at < self.freq.len() {
            let end = (at + WRITE_CHUNK).min(self.freq.len());
            for index in at..end {
                let freq = self.freq[index];
                self.feed(freq);
            }
            self.advance(out);
            at = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;
    use sdrmm_wire::{ChannelParams, NfmParams, SstvMode, SstvParams};

    use super::{
        modes::{BREAK_MS, LEADER_MS},
        *,
    };
    use crate::{
        testgen::{
            self,
            sstv::{Frame, bars, header, transmission},
        },
        testutil::{complex_noise, settings},
    };

    const RATE: f64 = INPUT_RATE_HZ;
    const BLOCKS: [usize; 6] = [4_096, 1, 997, 65_536, 33, 12_288];

    fn channel(p: SstvParams) -> SstvChannel {
        SstvChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Sstv(p)),
        )
        .expect("builds")
    }

    struct Received {
        images: Vec<DecodedImage>,
        events: Vec<SstvPicture>,
        progress: usize,
    }

    fn run(chan: &mut SstvChannel, iq: &[Complex<f32>], lens: &[usize]) -> Received {
        let mut out = ChannelOutputs::default();
        let mut received = Received {
            images: Vec::new(),
            events: Vec::new(),
            progress: 0,
        };
        let mut at = 0;
        for len in lens.iter().cycle() {
            if at >= iq.len() {
                break;
            }
            let end = (at + len).min(iq.len());
            out.reset();
            chan.process(&iq[at..end], &mut out);
            received.progress += out.video.len();
            received.images.append(&mut out.images);
            for event in &out.events {
                match event {
                    DecoderEvent::Sstv(picture) => received.events.push(*picture),
                    other => panic!("unexpected event {other:?}"),
                }
            }
            at = end;
        }
        received
    }

    fn tail(ms: f64) -> Vec<Complex<f32>> {
        testgen::silence(samples(ms, RATE) as usize)
    }

    fn decode(mode: SstvMode, frame: &Frame) -> DecodedImage {
        let mut iq = transmission(mode, frame, RATE);
        iq.extend_from_slice(&tail(2_000.0));
        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(
            received.images.len(),
            1,
            "{mode:?} produced no single image"
        );
        assert!(
            received.progress > 1,
            "{mode:?} sent no progressive updates"
        );
        received.images.into_iter().next().expect("one image")
    }

    fn mean_error(mode: SstvMode, sent: &Frame, got: &VideoPicture) -> f64 {
        let (width, height) = mode.size();
        assert_eq!((got.width, got.height), (width, height));
        let mut sum = 0.0f64;
        let mut count = 0u64;
        for y in 0..height {
            for x in 0..width {
                let base = (usize::from(y) * usize::from(width) + usize::from(x)) * 3;
                for (channel, want) in sent.pixel(x, y).into_iter().enumerate() {
                    sum += f64::from(got.rgb[base + channel].abs_diff(want));
                    count += 1;
                }
            }
        }
        sum / count as f64
    }

    fn edges(mode: SstvMode) -> Vec<u16> {
        let (width, _) = mode.size();
        (1..8)
            .map(|bar| (u32::from(width) * bar / 8) as u16)
            .collect()
    }

    fn away_from_edges(mode: SstvMode, x: u16) -> bool {
        let guard = (mode.size().0 / 64).max(3);
        edges(mode).into_iter().all(|edge| x.abs_diff(edge) > guard)
    }

    fn interior_error(mode: SstvMode, sent: &Frame, got: &VideoPicture) -> f64 {
        let (width, height) = mode.size();
        let mut sum = 0.0f64;
        let mut count = 0u64;
        for y in 0..height {
            for x in 0..width {
                if !away_from_edges(mode, x) {
                    continue;
                }
                let base = (usize::from(y) * usize::from(width) + usize::from(x)) * 3;
                for (channel, want) in sent.pixel(x, y).into_iter().enumerate() {
                    sum += f64::from(got.rgb[base + channel].abs_diff(want));
                    count += 1;
                }
            }
        }
        sum / count as f64
    }

    #[test]
    fn the_track_holds_the_longest_line_plus_a_write_chunk() {
        let longest = samples(modes::longest_line_ms(), RATE) as usize;
        let search = samples(SEARCH_MS, RATE) as usize;
        assert!(
            longest + 2 * search + WRITE_CHUNK < TRACK_CAPACITY,
            "a {longest}-sample line plus a {WRITE_CHUNK}-sample write does not fit {TRACK_CAPACITY}"
        );
    }

    #[test]
    fn a_vis_header_names_its_mode() {
        for &mode in &SstvMode::ALL {
            let mut iq = header(mode, RATE);
            iq.extend_from_slice(&tail(50.0));
            let mut chan = channel(SstvParams::default());
            let mut out = ChannelOutputs::default();
            chan.process(&iq, &mut out);
            assert!(
                chan.picture.active,
                "{mode:?} header did not start a picture"
            );
            assert_eq!(chan.picture.timing.mode, mode);
        }
    }

    #[test]
    fn a_header_with_broken_parity_starts_nothing() {
        let mut iq = header(SstvMode::MartinM1, RATE);
        let bit = samples(VIS_BIT_MS, RATE) as usize;
        let leaders = samples(LEADER_MS * 2.0 + BREAK_MS, RATE) as usize;
        let parity = leaders + bit * 8;
        let flipped = testgen::sstv::header(SstvMode::MartinM2, RATE);
        iq[parity..parity + bit].copy_from_slice(&flipped[parity..parity + bit]);
        iq.extend_from_slice(&tail(50.0));

        let mut chan = channel(SstvParams::default());
        let mut out = ChannelOutputs::default();
        chan.process(&iq, &mut out);
        assert!(
            !chan.picture.active,
            "a corrupted VIS still started a picture"
        );
    }

    #[test]
    fn every_mode_decodes_its_own_transmission() {
        for &mode in &SstvMode::ALL {
            let sent = bars(mode);
            let image = decode(mode, &sent);
            assert!(image.complete, "{mode:?} did not complete");
            assert_eq!(image.source, SOURCE);
            assert_eq!(image.mode, mode.label());
            assert_eq!(image.lines, mode.size().1);
            let error = interior_error(mode, &sent, &image.picture);
            assert!(error < 12.0, "{mode:?} mean interior error {error:.1}/255");
        }
    }

    #[test]
    fn a_decoded_picture_reports_its_mode_and_size() {
        let mode = SstvMode::MartinM1;
        let mut iq = transmission(mode, &bars(mode), RATE);
        iq.extend_from_slice(&tail(2_000.0));
        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(received.events.len(), 1);
        let event = received.events[0];
        assert_eq!(event.mode, mode);
        assert_eq!((event.width, event.height), mode.size());
        assert_eq!(event.lines, 256);
        assert!(event.complete);
        assert_eq!(event.seq, 1);
        let expected = timing(mode).seconds() * 1_000.0;
        let actual = f64::from(event.duration_ms);
        assert!(
            (actual - expected).abs() < 2_000.0,
            "reported {actual} ms against a {expected} ms transmission"
        );
    }

    #[test]
    fn ragged_block_splits_decode_identically() {
        let mode = SstvMode::Robot36;
        let sent = bars(mode);
        let mut iq = transmission(mode, &sent, RATE);
        iq.extend_from_slice(&tail(2_000.0));

        let whole = run(&mut channel(SstvParams::default()), &iq, &[iq.len()]);
        let ragged = run(&mut channel(SstvParams::default()), &iq, &BLOCKS);
        assert_eq!(whole.images.len(), 1);
        assert_eq!(ragged.images.len(), 1);
        assert_eq!(whole.images[0].picture, ragged.images[0].picture);
    }

    #[test]
    fn a_forced_mode_ignores_the_transmitted_vis() {
        let sent = bars(SstvMode::MartinM1);
        let mut iq = transmission(SstvMode::MartinM1, &sent, RATE);
        iq.extend_from_slice(&tail(2_000.0));
        let mut chan = channel(SstvParams {
            mode: Some(SstvMode::MartinM2),
            ..SstvParams::default()
        });
        let received = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(received.images.len(), 1);
        assert_eq!(received.images[0].mode, SstvMode::MartinM2.label());
    }

    #[test]
    fn a_transmission_cut_short_is_kept_as_a_partial_picture() {
        let mode = SstvMode::Robot36;
        let sent = bars(mode);
        let full = transmission(mode, &sent, RATE);
        let half = full.len() / 2;
        let mut iq = full[..half].to_vec();
        iq.extend_from_slice(&testgen::silence(samples(20_000.0, RATE) as usize));

        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(received.images.len(), 1);
        let image = &received.images[0];
        assert!(
            !image.complete,
            "a truncated picture claimed to be complete"
        );
        assert!(
            (100..200).contains(&image.lines),
            "kept {} lines of a half transmission",
            image.lines
        );
        assert!(!received.events[0].complete);
    }

    #[test]
    fn dropping_partials_keeps_only_finished_pictures() {
        let mode = SstvMode::Robot36;
        let full = transmission(mode, &bars(mode), RATE);
        let mut iq = full[..full.len() / 2].to_vec();
        iq.extend_from_slice(&testgen::silence(samples(20_000.0, RATE) as usize));

        let mut chan = channel(SstvParams {
            keep_partial: false,
            ..SstvParams::default()
        });
        let received = run(&mut chan, &iq, &BLOCKS);
        assert!(received.images.is_empty(), "a partial survived the setting");
    }

    #[test]
    fn a_second_transmission_follows_the_first() {
        let mode = SstvMode::Robot36;
        let sent = bars(mode);
        let mut iq = transmission(mode, &sent, RATE);
        iq.extend_from_slice(&tail(500.0));
        iq.extend(transmission(mode, &sent, RATE));
        iq.extend_from_slice(&tail(2_000.0));

        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(received.images.len(), 2);
        assert!(received.images.iter().all(|image| image.complete));
        assert_eq!(received.events[0].seq, 1);
        assert_eq!(received.events[1].seq, 2);
    }

    #[test]
    fn a_slanted_clock_still_lands_in_frame() {
        let mode = SstvMode::MartinM2;
        let sent = bars(mode);
        let straight = transmission(mode, &sent, RATE);
        let slanted = testgen::resample(&straight, RATE, RATE * 1.0005);
        let mut iq = slanted;
        iq.extend_from_slice(&tail(2_000.0));

        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(received.images.len(), 1);
        let error = interior_error(mode, &sent, &received.images[0].picture);
        assert!(error < 20.0, "slanted mean interior error {error:.1}/255");

        let mut chan = channel(SstvParams {
            slant_correction: false,
            ..SstvParams::default()
        });
        let free = run(&mut chan, &iq, &BLOCKS);
        assert_eq!(free.images.len(), 1);
        let uncorrected = interior_error(mode, &sent, &free.images[0].picture);
        assert!(
            uncorrected > error,
            "correction made it worse: {error:.1} against {uncorrected:.1}"
        );
    }

    #[test]
    fn decodes_through_additive_noise() {
        let mode = SstvMode::MartinM2;
        let sent = bars(mode);
        let mut iq = transmission(mode, &sent, RATE);
        testgen::add_noise(&mut iq, 0xabad_1dea, 0.25);
        let mut filtered = Vec::new();
        channel_filter(&SstvParams::default())
            .expect("filter")
            .process(&iq, &mut filtered);
        filtered.extend_from_slice(&tail(2_000.0));

        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &filtered, &BLOCKS);
        assert_eq!(received.images.len(), 1);
        let error = interior_error(mode, &sent, &received.images[0].picture);
        assert!(error < 25.0, "noisy mean interior error {error:.1}/255");
    }

    #[test]
    fn pure_noise_decodes_to_nothing() {
        for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
            let noise = complex_noise(seed, 0.4, 400_000);
            let mut chan = channel(SstvParams::default());
            let received = run(&mut chan, &noise, &BLOCKS);
            assert!(
                received.images.is_empty(),
                "seed {seed:#x} produced {} images",
                received.images.len()
            );
        }
    }

    #[test]
    fn a_grey_wedge_survives_the_round_trip() {
        let mode = SstvMode::MartinM1;
        let (width, height) = mode.size();
        let mut sent = Frame::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let level = (u32::from(x) * 255 / u32::from(width - 1)) as u8;
                sent.set(x, y, [level, level, level]);
            }
        }
        let image = decode(mode, &sent);
        let error = mean_error(mode, &sent, &image.picture);
        assert!(error < 6.0, "grey wedge mean error {error:.1}/255");
    }

    #[test]
    fn retuning_drops_the_picture_in_flight() {
        let mode = SstvMode::MartinM1;
        let iq = transmission(mode, &bars(mode), RATE);
        let mut chan = channel(SstvParams::default());
        let mut out = ChannelOutputs::default();
        chan.process(&iq[..iq.len() / 4], &mut out);
        assert!(chan.picture.active);
        chan.retuned();
        assert!(!chan.picture.active);
        out.reset();
        chan.process(&iq[iq.len() / 4..], &mut out);
        assert!(out.images.is_empty(), "a retuned channel still emitted");
    }

    #[test]
    fn mismatched_params_variant_is_rejected() {
        let mut chan = channel(SstvParams::default());
        let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
        assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
        let built = SstvChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn wrong_input_rate_is_rejected() {
        let built = SstvChannel::new(
            ChannelCtx {
                input_rate: 48_000.0,
            },
            settings(ChannelParams::Sstv(SstvParams::default())),
        );
        assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
    }

    #[test]
    fn changing_the_forced_mode_abandons_the_picture() {
        let mode = SstvMode::MartinM1;
        let iq = transmission(mode, &bars(mode), RATE);
        let mut chan = channel(SstvParams::default());
        let mut out = ChannelOutputs::default();
        chan.process(&iq[..iq.len() / 4], &mut out);
        assert!(chan.picture.active);
        chan.apply(settings(ChannelParams::Sstv(SstvParams {
            mode: Some(SstvMode::ScottieS1),
            ..SstvParams::default()
        })))
        .expect("applies");
        assert!(!chan.picture.active);
    }
}
