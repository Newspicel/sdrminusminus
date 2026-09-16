use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use ffmpeg_the_third::codec::Id;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_wire::DecoderEvent;

use crate::{
    AUDIO_RATE, ChannelError, ChannelOutputs, VideoPicture,
    dab::{
        packet::{Config as PacketConfig, PacketData},
        pad::{Event as PadEvent, Pad},
        superframe::AudioFormat,
    },
};

mod clock;
mod decoder;
mod latm;

#[cfg(test)]
mod tests;

const CHUNK_BYTES: usize = 8192;
const INPUT_SLOTS: usize = 256;
const OUTPUT_SLOTS: usize = 32;
const OUTPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mp2,
    Eac3,
    DabPad,
    DabPacket,
    Aac,
    Latm,
    Ac3,
    Mpeg2,
    H264,
    H265,
}

impl Kind {
    fn id(self) -> Id {
        match self {
            Self::Mp2 => Id::MP2,
            Self::Eac3 => Id::EAC3,
            Self::DabPad | Self::DabPacket => Id::None,
            Self::Aac => Id::AAC,
            Self::Latm => Id::AAC_LATM,
            Self::Ac3 => Id::AC3,
            Self::Mpeg2 => Id::MPEG2VIDEO,
            Self::H264 => Id::H264,
            Self::H265 => Id::HEVC,
        }
    }
    fn video(self) -> bool {
        matches!(self, Self::Mpeg2 | Self::H264 | Self::H265)
    }
}

struct Input {
    epoch: u64,
    kind: Kind,
    pts: Option<i64>,
    format: Option<AudioFormat>,
    mot_app: Option<u8>,
    packet: Option<PacketConfig>,
    length: usize,
    bytes: [u8; CHUNK_BYTES],
}

enum Payload {
    Data(PadEvent),
    Audio(Vec<f32>),
    Video(VideoPicture),
    Error { video: bool, message: String },
}

struct Output {
    epoch: u64,
    pts: Option<i64>,
    payload: Payload,
}

impl Output {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match &self.payload {
                Payload::Audio(pcm) => pcm.capacity() * 4,
                Payload::Video(picture) => picture.rgb.capacity() + picture.luma.capacity(),
                Payload::Data(PadEvent::Object(object)) => {
                    object.bytes.capacity()
                        + object.name.capacity()
                        + object.media_type.capacity()
                        + object.label.capacity()
                }
                Payload::Data(PadEvent::Label(label)) => label.capacity(),
                Payload::Data(PadEvent::Error(_)) => 0,
                Payload::Error { message, .. } => message.capacity(),
            }
    }
}

fn publish(output: &mut Producer<Output>, resident: &AtomicUsize, mut value: Output) -> bool {
    let bytes = value.bytes();
    while resident.load(Ordering::Acquire) + bytes > OUTPUT_BYTES {
        if output.is_abandoned() {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    resident.fetch_add(bytes, Ordering::AcqRel);
    while let Err(PushError::Full(held)) = output.push(value) {
        if output.is_abandoned() {
            return false;
        }
        value = held;
        thread::sleep(Duration::from_millis(1));
    }
    true
}

fn run(mut input: Consumer<Input>, mut output: Producer<Output>, resident: Arc<AtomicUsize>) {
    let mut epoch = 0;
    let mut audio = None;
    let mut video = None;
    let mut frames = Vec::new();
    let mut pad = Pad::default();
    let mut pad_events = Vec::new();
    let mut mp2 = Vec::new();
    let mut packet: Option<(PacketConfig, PacketData)> = None;
    loop {
        if output.is_abandoned() {
            return;
        }
        let item = match input.pop() {
            Ok(item) => item,
            Err(_) if input.is_abandoned() => return,
            Err(_) => {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
        };
        if epoch != item.epoch {
            audio = None;
            video = None;
            pad = Pad::default();
            mp2.clear();
            packet = None;
            epoch = item.epoch;
        }
        pad.mot_app = item.mot_app;
        if item.kind == Kind::DabPacket {
            if let Some(config) = item.packet {
                if packet
                    .as_ref()
                    .is_none_or(|(current, _)| *current != config)
                {
                    packet = Some((config, PacketData::new(config)));
                }
                if let Some((_, decoder)) = &mut packet {
                    decoder.push(&item.bytes[..item.length], &mut pad_events);
                }
            } else {
                pad_events.push(PadEvent::Error("DAB packet configuration missing"));
            }
        } else if item.format.is_some() {
            pad.access_unit(&item.bytes[..item.length], &mut pad_events);
        } else if item.kind == Kind::DabPad {
            mp2.extend_from_slice(&item.bytes[..item.length]);
            while mp2.len() >= 4 {
                let Some(header) = crate::broadcast_audio::Header::read(&mp2) else {
                    mp2.remove(0);
                    continue;
                };
                if mp2.len() < header.length {
                    break;
                }
                let n = header.length;
                let crc_length =
                    if header.mpeg1 && header.bitrate < if header.mono { 56 } else { 112 } {
                        2
                    } else {
                        4
                    };
                if n >= crc_length + 2 {
                    pad.process(
                        &mp2[..n - crc_length - 2],
                        [mp2[n - 2], mp2[n - 1]],
                        false,
                        &mut pad_events,
                    );
                }
                mp2.drain(..n);
            }
        }
        for event in pad_events.drain(..) {
            if !publish(
                &mut output,
                &resident,
                Output {
                    epoch,
                    pts: None,
                    payload: Payload::Data(event),
                },
            ) {
                return;
            }
        }
        if matches!(item.kind, Kind::DabPad | Kind::DabPacket) {
            continue;
        }
        let state: &mut Option<(Kind, decoder::Decoder)> = if item.kind.video() {
            &mut video
        } else {
            &mut audio
        };
        let result = (|| {
            if state.as_ref().is_none_or(|(kind, _)| *kind != item.kind) {
                *state = Some((item.kind, decoder::Decoder::new(item.kind)?));
            }
            let Some((_, decoder)) = state else {
                return Err("Broadcast decoder missing".to_owned());
            };
            let bytes = &item.bytes[..item.length];
            let latm;
            let encoded = if let Some(format) = item.format {
                latm = latm::wrap(bytes, format).map_err(str::to_owned)?;
                &latm
            } else {
                bytes
            };
            decoder.push(encoded, item.pts, &mut frames)
        })();
        for (pts, payload) in frames.drain(..) {
            if !publish(
                &mut output,
                &resident,
                Output {
                    epoch,
                    pts,
                    payload,
                },
            ) {
                return;
            }
        }
        if let Err(mut message) = result {
            if let Some((_, decoder)) = state
                && let Err(error) = decoder.recover()
            {
                message.push_str(&format!("; {error}"));
                *state = None;
            }
            if !publish(
                &mut output,
                &resident,
                Output {
                    epoch,
                    pts: item.pts,
                    payload: Payload::Error {
                        video: item.kind.video(),
                        message,
                    },
                },
            ) {
                return;
            }
        }
    }
}

pub struct BroadcastMedia {
    input: Producer<Input>,
    resident: Arc<AtomicUsize>,
    pub service_id: Option<u32>,
    output: Consumer<Output>,
    epoch: u64,
    clock: Option<clock::Clock>,
    pending: VecDeque<Output>,
    pub mot_app: Option<u8>,
    pub packet_config: Option<PacketConfig>,
    pub dynamic_label: Option<String>,
    pub data_groups: u32,
    pub data_errors: u32,
    pub data_error: Option<String>,
    pub audio_frames: u32,
    pub video_frames: u32,
    pub audio_errors: u32,
    pub video_errors: u32,
    pub audio_error: Option<String>,
    pub video_error: Option<String>,
}

impl BroadcastMedia {
    pub fn new() -> Result<Self, ChannelError> {
        let (input, incoming) = RingBuffer::new(INPUT_SLOTS);
        let (outgoing, output) = RingBuffer::new(OUTPUT_SLOTS);
        let resident = Arc::new(AtomicUsize::new(0));
        let worker_resident = resident.clone();
        thread::Builder::new()
            .name("broadcast-media".to_owned())
            .spawn(move || run(incoming, outgoing, worker_resident))
            .map_err(|error| ChannelError::InvalidSettings(format!("Media worker: {error}")))?;
        Ok(Self {
            input,
            resident,
            service_id: None,
            output,
            epoch: 0,
            clock: None,
            pending: VecDeque::with_capacity(OUTPUT_SLOTS),
            mot_app: None,
            packet_config: None,
            dynamic_label: None,
            data_groups: 0,
            data_errors: 0,
            data_error: None,
            audio_frames: 0,
            video_frames: 0,
            audio_errors: 0,
            video_errors: 0,
            audio_error: None,
            video_error: None,
        })
    }

    pub fn enable_clock(&mut self) {
        self.clock = Some(clock::Clock::default());
    }

    pub fn advance(&mut self, samples: usize, rate: f64) {
        if let Some(clock) = &mut self.clock {
            clock.advance(samples, rate);
        }
    }

    pub fn reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.clock.is_some() {
            self.enable_clock();
        }
        for item in self.pending.drain(..) {
            self.resident.fetch_sub(item.bytes(), Ordering::AcqRel);
        }
        self.service_id = None;
        self.dynamic_label = None;
        self.data_groups = 0;
        self.data_errors = 0;
        self.data_error = None;
        self.audio_frames = 0;
        self.video_frames = 0;
        self.audio_errors = 0;
        self.video_errors = 0;
        self.audio_error = None;
        self.video_error = None;
    }

    pub fn audio_gap(&mut self, count: u32) {
        self.epoch = self.epoch.wrapping_add(1);
        self.audio_errors = self.audio_errors.saturating_add(count);
        self.audio_error = Some("DAB+ access-unit CRC failure".to_owned());
    }

    fn failed(&mut self, video: bool, error: String) {
        if video {
            self.video_errors = self.video_errors.saturating_add(1);
            self.video_error = Some(error);
        } else {
            self.audio_errors = self.audio_errors.saturating_add(1);
            self.audio_error = Some(error);
        }
    }

    fn input_failed(&mut self, kind: Kind, message: &str) {
        if matches!(kind, Kind::DabPad | Kind::DabPacket) {
            self.data_errors = self.data_errors.saturating_add(1);
            self.data_error = Some(message.to_owned());
        } else {
            self.failed(kind.video(), message.to_owned());
        }
    }

    pub fn push(
        &mut self,
        kind: Kind,
        bytes: &[u8],
        pts: Option<u64>,
        format: Option<AudioFormat>,
    ) {
        if self.input.slots() < bytes.len().div_ceil(CHUNK_BYTES)
            || (format.is_some() && bytes.len() > CHUNK_BYTES)
        {
            self.input_failed(kind, "Broadcast media input overflow");
            self.epoch = self.epoch.wrapping_add(1);
            return;
        }
        for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
            let mut item = Input {
                epoch: self.epoch,
                kind,
                pts: pts.filter(|_| index == 0).map(|v| v as i64),
                format,
                mot_app: self.mot_app,
                packet: self.packet_config,
                length: chunk.len(),
                bytes: [0; CHUNK_BYTES],
            };
            item.bytes[..chunk.len()].copy_from_slice(chunk);
            if self.input.push(item).is_err() {
                self.input_failed(kind, "Broadcast media worker stopped");
                return;
            }
        }
    }

    pub fn drain(&mut self, out: &mut ChannelOutputs) {
        while self.pending.len() < OUTPUT_SLOTS {
            let Ok(item) = self.output.pop() else { break };
            self.pending.push_back(item);
        }
        for _ in 0..self.pending.len() {
            let Some(item) = self.pending.pop_front() else {
                break;
            };
            if item.epoch != self.epoch {
                self.resident.fetch_sub(item.bytes(), Ordering::AcqRel);
                continue;
            }
            if let (Some(clock), Some(pts)) = (&mut self.clock, item.pts)
                && !clock.due(pts)
            {
                self.pending.push_back(item);
                continue;
            }
            self.resident.fetch_sub(item.bytes(), Ordering::AcqRel);
            match item.payload {
                Payload::Data(event) => match event {
                    PadEvent::Label(text) => {
                        self.dynamic_label = Some(text);
                        self.data_groups = self.data_groups.saturating_add(1);
                    }
                    PadEvent::Object(mut object) => {
                        object.service_id = self.service_id;
                        self.data_groups = self.data_groups.saturating_add(1);
                        out.events.push(DecoderEvent::BroadcastData(object));
                    }
                    PadEvent::Error(error) => {
                        self.data_errors = self.data_errors.saturating_add(1);
                        self.data_error = Some(error.to_owned());
                    }
                },
                Payload::Audio(pcm) => {
                    out.audio_rate = AUDIO_RATE;
                    out.audio_pcm.extend_from_slice(&pcm);
                    self.audio_frames = self.audio_frames.saturating_add(1);
                }
                Payload::Video(picture) => {
                    out.video.push(picture);
                    self.video_frames = self.video_frames.saturating_add(1);
                }
                Payload::Error { video, message } => self.failed(video, message),
            }
        }
    }
}
