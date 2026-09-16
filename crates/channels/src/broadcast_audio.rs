use std::{thread, time::Duration};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_dsp::FracResampler;
use symphonia_bundle_mp3::MpaDecoder;
use symphonia_core::{
    audio::{Audio, GenericAudioBufferRef},
    codecs::audio::{
        AudioCodecParameters, AudioDecoder, AudioDecoderOptions, well_known::CODEC_ID_MP2,
    },
    packet::PacketRef,
    units::{Duration as MediaDuration, Timestamp},
};

use crate::{AUDIO_RATE, ChannelError, ChannelOutputs};

mod crc;

const FRAME_BYTES: usize = 1729;
const PCM_SAMPLES: usize = 6920;
const QUEUE_FRAMES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    length: usize,
    rate: u32,
    mono: bool,
    bitrate: u32,
    mpeg1: bool,
}

impl Header {
    fn read(bytes: &[u8]) -> Option<Self> {
        let &[a, b, c, d, ..] = bytes else {
            return None;
        };
        let version = (b >> 3) & 3;
        if a != 0xff || b & 0xe0 != 0xe0 || !matches!(version, 2 | 3) || b & 6 != 4 {
            return None;
        }
        let rates = [44100, 48000, 32000];
        let rate = *rates.get(usize::from((c >> 2) & 3))? / if version == 3 { 1 } else { 2 };
        let table = if version == 3 {
            [
                0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
            ]
        } else {
            [
                0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
            ]
        };
        let bitrate = table[usize::from(c >> 4)];
        let length = (144000 * bitrate / rate) as usize + usize::from((c >> 1) & 1);
        (bitrate > 0 && (4..=FRAME_BYTES).contains(&length)).then_some(Self {
            length,
            rate,
            mono: d >> 6 == 3,
            bitrate,
            mpeg1: version == 3,
        })
    }
}

struct Encoded {
    epoch: u64,
    discontinuity: bool,
    header: Header,
    bytes: [u8; FRAME_BYTES],
}

struct Decoded {
    epoch: u64,
    length: usize,
    pcm: [f32; PCM_SAMPLES],
    error: Option<&'static str>,
}

struct Decoder {
    codec: MpaDecoder,
    spec: Option<(u32, bool)>,
    resampler: Option<FracResampler>,
    input: Vec<Complex<f32>>,
    output: Vec<Complex<f32>>,
}

fn codec() -> Result<MpaDecoder, ChannelError> {
    let mut params = AudioCodecParameters::new();
    params.codec = CODEC_ID_MP2;
    MpaDecoder::try_new(&params, &AudioDecoderOptions::default())
        .map_err(|error| ChannelError::InvalidSettings(format!("MPEG Layer II decoder: {error}")))
}

impl Decoder {
    fn new() -> Result<Self, ChannelError> {
        Ok(Self {
            codec: codec()?,
            spec: None,
            resampler: None,
            input: Vec::with_capacity(1152),
            output: Vec::with_capacity(3456),
        })
    }

    fn reset(&mut self) {
        self.spec = None;
    }

    fn decode(&mut self, encoded: &Encoded, decoded: &mut Decoded) -> Result<(), &'static str> {
        if !crc::valid(&encoded.bytes[..encoded.header.length], encoded.header) {
            return Err("MPEG Layer II audio CRC mismatch");
        }
        let spec = (encoded.header.rate, encoded.header.mono);
        if self.spec != Some(spec) {
            self.codec = codec().map_err(|_| "MPEG Layer II decoder initialization failed")?;
            self.resampler = (spec.0 != AUDIO_RATE)
                .then(|| FracResampler::new(f64::from(AUDIO_RATE) / f64::from(spec.0)));
            self.spec = Some(spec);
        }
        let packet = PacketRef::new(
            0,
            Timestamp::ZERO,
            MediaDuration::ZERO,
            &encoded.bytes[..encoded.header.length],
        );
        let GenericAudioBufferRef::F32(audio) = self
            .codec
            .decode_ref(&packet)
            .map_err(|_| "Invalid MPEG Layer II audio frame")?
        else {
            return Err("Unexpected MPEG Layer II sample format");
        };
        let left = audio
            .plane(0)
            .ok_or("MPEG Layer II frame has no audio channels")?;
        let right = audio.plane(1).unwrap_or(left);
        self.input.clear();
        self.input
            .extend(left.iter().zip(right).map(|(&l, &r)| Complex::new(l, r)));
        let samples = if let Some(resampler) = &mut self.resampler {
            resampler.process(&self.input, &mut self.output);
            &self.output
        } else {
            &self.input
        };
        if samples.len() * 2 > decoded.pcm.len() {
            return Err("MPEG Layer II audio output exceeds frame capacity");
        }
        if samples
            .iter()
            .any(|s| !s.re.is_finite() || !s.im.is_finite())
        {
            return Err("MPEG Layer II decoder produced non-finite audio");
        }
        decoded.length = samples.len() * 2;
        for (pair, sample) in decoded.pcm[..decoded.length]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(samples)
        {
            pair[0] = sample.re.clamp(-1.0, 1.0);
            pair[1] = sample.im.clamp(-1.0, 1.0);
        }
        Ok(())
    }
}

fn run(mut decoder: Decoder, mut input: Consumer<Encoded>, mut output: Producer<Decoded>) {
    let mut epoch = 0;
    loop {
        if output.is_abandoned() {
            return;
        }
        let encoded = match input.pop() {
            Ok(encoded) => encoded,
            Err(_) if input.is_abandoned() => return,
            Err(_) => {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
        };
        if encoded.epoch != epoch || encoded.discontinuity {
            decoder.reset();
            epoch = encoded.epoch;
        }
        let mut decoded = Decoded {
            epoch,
            length: 0,
            pcm: [0.0; PCM_SAMPLES],
            error: None,
        };
        if let Err(error) = decoder.decode(&encoded, &mut decoded) {
            decoded.error = Some(error);
            decoder.reset();
        }
        while let Err(PushError::Full(held)) = output.push(decoded) {
            if output.is_abandoned() {
                return;
            }
            decoded = held;
            thread::sleep(Duration::from_millis(1));
        }
    }
}

pub struct LayerTwoAudio {
    input: Producer<Encoded>,
    output: Consumer<Decoded>,
    bytes: [u8; FRAME_BYTES],
    used: usize,
    header: Option<Header>,
    epoch: u64,
    lost_sync: bool,
    discontinuity: bool,
    pub frames_ok: u32,
    pub frames_bad: u32,
    pub error: Option<&'static str>,
}

impl LayerTwoAudio {
    pub fn new() -> Result<Self, ChannelError> {
        let decoder = Decoder::new()?;
        let (input, incoming) = RingBuffer::new(QUEUE_FRAMES);
        let (outgoing, output) = RingBuffer::new(QUEUE_FRAMES);
        thread::Builder::new()
            .name("broadcast-audio".to_owned())
            .spawn(move || run(decoder, incoming, outgoing))
            .map_err(|error| {
                ChannelError::InvalidSettings(format!("Broadcast audio worker: {error}"))
            })?;
        Ok(Self {
            input,
            output,
            bytes: [0; FRAME_BYTES],
            used: 0,
            header: None,
            epoch: 0,
            lost_sync: false,
            discontinuity: false,
            frames_ok: 0,
            frames_bad: 0,
            error: None,
        })
    }

    pub fn reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.used = 0;
        self.header = None;
        self.lost_sync = false;
        self.discontinuity = false;
        self.frames_ok = 0;
        self.frames_bad = 0;
        self.error = None;
    }

    fn failed(&mut self, reason: &'static str) {
        self.frames_bad = self.frames_bad.saturating_add(1);
        self.error = Some(reason);
    }

    pub fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.bytes[self.used] = byte;
            self.used += 1;
            if self.used < 4 {
                continue;
            }
            if self.header.is_none() {
                self.header = Header::read(&self.bytes[..4]);
                if self.header.is_none() {
                    self.bytes.copy_within(1..4, 0);
                    self.used = 3;
                    if !self.lost_sync {
                        self.failed("Lost MPEG Layer II frame synchronization");
                    }
                    self.lost_sync = true;
                    self.discontinuity = true;
                    continue;
                }
                self.lost_sync = false;
            }
            if let Some(header) = self.header
                && self.used == header.length
            {
                let frame = Encoded {
                    epoch: self.epoch,
                    discontinuity: self.discontinuity,
                    header,
                    bytes: self.bytes,
                };
                if self.input.push(frame).is_err() {
                    self.failed("Broadcast audio input queue overflow");
                    self.discontinuity = true;
                } else {
                    self.discontinuity = false;
                }
                self.used = 0;
                self.header = None;
            }
        }
    }

    pub fn drain(&mut self, out: &mut ChannelOutputs) {
        while let Ok(decoded) = self.output.pop() {
            if decoded.epoch != self.epoch {
                continue;
            }
            if let Some(error) = decoded.error {
                self.failed(error);
                continue;
            }
            out.audio_rate = AUDIO_RATE;
            out.audio_pcm
                .extend_from_slice(&decoded.pcm[..decoded.length]);
            self.frames_ok = self.frames_ok.saturating_add(1);
        }
        if self.input.is_abandoned() && self.error != Some("Broadcast audio worker stopped") {
            self.failed("Broadcast audio worker stopped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO: &[u8] = include_bytes!("../../../fixtures/broadcast_audio/tone_48k_mono.mp2");
    const STEREO: &[u8] = include_bytes!("../../../fixtures/broadcast_audio/tone_32k_stereo.mp2");

    fn wait_for(audio: &mut LayerTwoAudio, frames: u32, out: &mut ChannelOutputs) {
        let limit = std::time::Instant::now() + Duration::from_secs(3);
        while audio.frames_ok + audio.frames_bad < frames {
            assert!(
                std::time::Instant::now() < limit,
                "audio worker did not return {frames} frames"
            );
            audio.drain(out);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn mono_mpeg_layer_two_matches_the_independent_decoded_reference() {
        let mut audio = LayerTwoAudio::new().expect("audio worker");
        for block in MONO.chunks(17) {
            audio.push(block);
        }
        let mut out = ChannelOutputs::default();
        wait_for(&mut audio, 20, &mut out);
        assert_eq!(audio.frames_ok, 20);
        assert_eq!(audio.frames_bad, 0);
        assert_eq!(out.audio_rate, AUDIO_RATE);
        let expected: Vec<f32> =
            include_bytes!("../../../fixtures/broadcast_audio/tone_48k_mono.f32")
                .as_chunks::<4>()
                .0
                .iter()
                .map(|sample| f32::from_le_bytes(*sample))
                .collect();
        assert_eq!(out.audio_pcm.len(), expected.len() * 2);
        let mut error = 0.0;
        let mut power = 0.0;
        for (pair, expected) in out.audio_pcm.as_chunks::<2>().0.iter().zip(expected) {
            assert_eq!(pair[0], pair[1]);
            error += f64::from((pair[0] - expected).powi(2));
            power += f64::from(expected.powi(2));
        }
        assert!(
            error / power < 1e-5,
            "relative mean square error {}",
            error / power
        );
    }

    #[test]
    fn stereo_is_resampled_without_mixing_the_channels() {
        let mut audio = LayerTwoAudio::new().expect("audio worker");
        audio.push(STEREO);
        let mut out = ChannelOutputs::default();
        wait_for(&mut audio, 10, &mut out);
        assert_eq!(audio.frames_ok, 10);
        assert_eq!(audio.frames_bad, 0);
        assert!(out.audio_pcm.len().abs_diff(10 * 1152 * 3) <= 2);
        let tone = |channel: usize, hz: f32| -> f32 {
            out.audio_pcm
                .as_chunks::<2>()
                .0
                .iter()
                .skip(2000)
                .enumerate()
                .map(|(n, pair)| {
                    Complex::from_polar(
                        pair[channel],
                        std::f32::consts::TAU * hz * n as f32 / AUDIO_RATE as f32,
                    )
                })
                .sum::<Complex<f32>>()
                .norm()
        };
        assert!(tone(0, 700.0) > 100.0 * tone(0, 1300.0));
        assert!(tone(1, 1300.0) > 100.0 * tone(1, 700.0));
    }

    #[test]
    fn damaged_headers_reacquire_and_queue_overflow_is_reported() {
        let mut audio = LayerTwoAudio::new().expect("audio worker");
        audio.push(&[0; 79]);
        audio.push(&MONO[..192]);
        let mut out = ChannelOutputs::default();
        wait_for(&mut audio, 2, &mut out);
        assert_eq!(audio.frames_ok, 1);
        assert_eq!(audio.frames_bad, 1);
        assert_eq!(
            audio.error,
            Some("Lost MPEG Layer II frame synchronization")
        );
        for _ in 0..100 {
            audio.push(MONO);
        }
        assert_eq!(audio.error, Some("Broadcast audio input queue overflow"));
        assert!(audio.frames_bad > 1);
        audio.reset();
        assert_eq!(audio.frames_bad, 0);
        assert_eq!(audio.used, 0);
        assert!(audio.error.is_none());
    }

    #[test]
    fn stale_audio_cannot_cross_a_service_change() {
        let mut audio = LayerTwoAudio::new().expect("audio worker");
        audio.push(MONO);
        audio.reset();
        audio.push(&STEREO[..576]);
        let mut out = ChannelOutputs::default();
        wait_for(&mut audio, 1, &mut out);
        assert_eq!(audio.frames_ok, 1);
        assert!(out.audio_pcm.len().abs_diff(3456) <= 2);
    }

    #[test]
    fn protected_audio_rejects_crc_damage_and_recovers() {
        let mut frame = [0; 192];
        frame[..6].copy_from_slice(&[0xff, 0xfc, 0x44, 0xc0, 0xfe, 0xb3]);
        let mut damaged = frame;
        damaged[6] ^= 1;
        let mut audio = LayerTwoAudio::new().expect("audio worker");
        audio.push(&damaged);
        audio.push(&frame);
        let mut out = ChannelOutputs::default();
        wait_for(&mut audio, 2, &mut out);
        assert_eq!(audio.frames_ok, 1);
        assert_eq!(audio.frames_bad, 1);
        assert_eq!(audio.error, Some("MPEG Layer II audio CRC mismatch"));
        assert_eq!(out.audio_pcm.len(), 2304);
        assert!(out.audio_pcm.iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn frame_loss_resets_audio_synthesis_history() {
        let mut audio = LayerTwoAudio::new().expect("audio worker");
        audio.push(&MONO[..384]);
        let mut out = ChannelOutputs::default();
        wait_for(&mut audio, 2, &mut out);
        let first = out.audio_pcm[..2304].to_vec();
        out.audio_pcm.clear();
        audio.push(&[0; 17]);
        audio.push(&MONO[..192]);
        wait_for(&mut audio, 4, &mut out);
        assert_eq!(audio.frames_ok, 3);
        assert_eq!(audio.frames_bad, 1);
        assert_eq!(out.audio_pcm, first);
    }

    #[test]
    fn layer_two_decoding_stays_ahead_of_real_time() {
        let mut decoder = Decoder::new().expect("decoder");
        let mut bytes = [0; FRAME_BYTES];
        bytes[..192].copy_from_slice(&MONO[..192]);
        let encoded = Encoded {
            epoch: 0,
            discontinuity: false,
            header: Header::read(&bytes).expect("header"),
            bytes,
        };
        let mut decoded = Decoded {
            epoch: 0,
            length: 0,
            pcm: [0.0; PCM_SAMPLES],
            error: None,
        };
        let start = std::time::Instant::now();
        for _ in 0..100 {
            decoder.decode(&encoded, &mut decoded).expect("audio frame");
        }
        assert!(start.elapsed().as_secs_f64() < 2.4);
    }

    #[test]
    fn reserved_mpeg_headers_are_rejected() {
        for header in [
            [0xff, 0xfd, 0, 0],
            [0xff, 0xfd, 0x4c, 0],
            [0xff, 0xed, 0x44, 0],
            [0xff, 0xfb, 0x44, 0],
        ] {
            assert!(Header::read(&header).is_none());
        }
    }
}
