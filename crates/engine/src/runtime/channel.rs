use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
    mpsc,
};

use num_complex::Complex;
use sdrmm_channels::{
    AUDIO_RATE, AudioChain, ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    ClickProfile, DecodedImage,
};
use sdrmm_dsp::{Ddc, LevelMeter, Squelch};
use sdrmm_wire::{ChannelSettings, DecoderEvent, PositionFix};
use tokio::sync::broadcast;

use super::DSP_BLOCK;
use crate::{
    audio::PcmBlock,
    audio_recording::AudioRecorderTap,
    iq::IqBlock,
    network_export::NetworkExportTap,
    publishing::channel::{ChannelPublisher, IQ_WANTED, SYMBOLS_WANTED},
    recording::RecorderTap,
    symbols::SymbolBlock,
    video::VideoPacket,
};

const SQUELCH_HYSTERESIS_DB: f32 = 6.0;
const SQUELCH_HOLD_S: f32 = 0.1;

pub(crate) struct RawDecoded {
    pub(crate) device_set: u32,
    pub(crate) channel: u32,
    pub(crate) freq_hz: f64,
    pub(crate) event: DecoderEvent,
}

pub(crate) struct RawImage {
    pub(crate) device_set: u32,
    pub(crate) channel: u32,
    pub(crate) freq_hz: f64,
    pub(crate) image: DecodedImage,
}

#[derive(Clone)]
pub(crate) struct DecodedSink {
    tx: mpsc::SyncSender<RawDecoded>,
    image_tx: mpsc::SyncSender<RawImage>,
    dropped: Arc<AtomicU64>,
    device_set: u32,
    channel: u32,
}

impl DecodedSink {
    pub(crate) fn note_lost(&self, count: u64) {
        self.dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn new(
        tx: mpsc::SyncSender<RawDecoded>,
        image_tx: mpsc::SyncSender<RawImage>,
        dropped: Arc<AtomicU64>,
        device_set: u32,
        channel: u32,
    ) -> Self {
        Self {
            tx,
            image_tx,
            dropped,
            device_set,
            channel,
        }
    }

    #[cfg(test)]
    pub(crate) fn null() -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        drop(rx);
        let (image_tx, image_rx) = mpsc::sync_channel(1);
        drop(image_rx);
        Self::new(tx, image_tx, Arc::new(AtomicU64::new(0)), 0, 0)
    }

    pub(crate) fn publish(&self, freq_hz: f64, event: DecoderEvent) {
        let record = RawDecoded {
            device_set: self.device_set,
            channel: self.channel,
            freq_hz,
            event,
        };
        if self.tx.try_send(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn publish_image(&self, freq_hz: f64, image: DecodedImage) {
        let raw = RawImage {
            device_set: self.device_set,
            channel: self.channel,
            freq_hz,
            image,
        };
        if self.image_tx.try_send(raw).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
pub(crate) struct ChannelSinks {
    pub(crate) publication: Arc<crate::metrics::QueueMetrics>,
    pub(crate) pcm_tx: broadcast::Sender<PcmBlock>,
    pub(crate) pcm_pos: Arc<AtomicU64>,
    pub(crate) video_tx: broadcast::Sender<VideoPacket>,
    pub(crate) video_pos: Arc<AtomicU64>,
    pub(crate) iq_tx: broadcast::Sender<IqBlock>,
    pub(crate) symbol_tx: broadcast::Sender<SymbolBlock>,
    pub(crate) level_db: Arc<AtomicU32>,
    pub(crate) peak_db: Arc<AtomicU32>,
    pub(crate) squelch_db: Arc<AtomicU32>,
}

pub(crate) struct ChannelHost {
    ddc: Ddc,
    filter: ChannelFilter,
    squelch: Squelch,
    threshold_db: Option<f32>,
    audio_rec: Option<AudioRecorderTap>,
    rx: Box<dyn ChannelRx>,
    audio: AudioChain,
    has_audio: bool,
    outputs: ChannelOutputs,
    scratch: Vec<Complex<f32>>,
    filtered: Vec<Complex<f32>>,
    pcm_per_input: f64,
    zero_carry: f64,
    audio_channels: u8,
    sinks: ChannelSinks,
    produces_video: bool,
    offset_hz: f64,
    decoded: DecodedSink,
    emits_events: bool,
    gated: Vec<Complex<f32>>,
    publisher: ChannelPublisher,
    baseband_rec: Option<RecorderTap>,
    baseband_export: Option<NetworkExportTap>,
    baseband_pos: u64,
    input_rate: f64,
    device_rate: f64,
    next_input: Option<u64>,
    recovering: bool,
    gap_baseband: f64,
    gap_audio: f64,
    position: Option<PositionFix>,
    meter: LevelMeter,
    lo_artifact_hz: Option<f64>,
}

impl ChannelHost {
    pub(crate) fn build(
        device_rate: f64,
        settings: &ChannelSettings,
        sinks: ChannelSinks,
        decoded: DecodedSink,
    ) -> Result<Box<Self>, ChannelError> {
        let type_id = settings.params.type_id();
        let descriptor = sdrmm_channels::descriptors()
            .into_iter()
            .find(|d| d.type_id == type_id)
            .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
        let input_rate = match descriptor.native_rate_range() {
            Some(_) => device_rate,
            None => descriptor.input_rate_hz,
        };
        let ddc = Ddc::new(device_rate, input_rate, settings.offset_hz)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        let filter = sdrmm_channels::channel_filter(&settings.params)?;
        let rx = sdrmm_channels::create(ChannelCtx { input_rate }, settings)?;
        let audio_channels = sdrmm_channels::audio_channels(&settings.params);
        let decodes = descriptor.decoder_kind.is_some();
        let emits_events = decodes && rx.needs_gated_input();
        let mut squelch = Squelch::new(
            input_rate,
            settings.squelch_db.unwrap_or(0.0),
            SQUELCH_HYSTERESIS_DB,
            SQUELCH_HOLD_S,
        );
        squelch.set_auto_margin_db(settings.squelch_auto_db);
        let publisher = ChannelPublisher::new(
            input_rate,
            settings,
            (DSP_BLOCK as f64 * input_rate / device_rate).ceil() as usize + 64,
            sinks.clone(),
            decoded.clone(),
        )
        .map_err(|error| ChannelError::InvalidSettings(format!("start publisher: {error}")))?;
        Ok(Box::new(Self {
            ddc,
            filter,
            squelch,
            threshold_db: settings.squelch_db,
            audio_rec: None,
            rx,
            audio: AudioChain::new(
                input_rate,
                audio_channels,
                &settings.audio,
                ClickProfile::for_params(&settings.params),
            ),
            has_audio: descriptor.has_audio,
            outputs: ChannelOutputs::default(),
            scratch: Vec::new(),
            filtered: Vec::new(),
            pcm_per_input: f64::from(AUDIO_RATE) / input_rate,
            zero_carry: 0.0,
            audio_channels,
            sinks,
            produces_video: descriptor.has_video,
            offset_hz: settings.offset_hz,
            decoded,
            emits_events,
            gated: Vec::new(),
            publisher,
            baseband_rec: None,
            baseband_export: None,
            baseband_pos: 0,
            input_rate,
            device_rate,
            next_input: None,
            recovering: false,
            gap_baseband: 0.0,
            gap_audio: 0.0,
            position: None,
            meter: LevelMeter::new(input_rate),
            lo_artifact_hz: None,
        }))
    }

    pub(super) fn inherit(&mut self, previous: &mut Self) {
        self.publisher.queue.follow(&previous.publisher.queue);
        self.audio_rec = previous.audio_rec.take();
        self.baseband_rec = previous.baseband_rec.take();
        self.baseband_export = previous.baseband_export.take();
        self.baseband_pos = previous.baseband_pos;
        self.next_input = previous.next_input;
        self.gap_audio = previous.gap_audio;
        self.gap_baseband = previous.gap_baseband;
    }

    pub(super) fn process_at(
        &mut self,
        input: &[Complex<f32>],
        index: u64,
        center_hz: f64,
        lo_offset_hz: f64,
    ) {
        if let Some(next) = self.next_input
            && next != index
        {
            self.skip_input(index.saturating_sub(next));
            self.recovering = true;
        }
        self.next_input = Some(index.saturating_add(input.len() as u64));
        if self.recovering {
            if !self.publisher.recover(&mut self.rx) {
                self.skip_input(input.len() as u64);
                return;
            }
            self.rx.position_changed(self.position.as_ref());
            self.ddc.reset();
            self.filter.reset();
            self.audio.reset();
            self.squelch.reset();
            self.lo_artifact_hz = None;
            self.recovering = false;
        }
        self.process(input, center_hz, lo_offset_hz);
    }

    fn skip_input(&mut self, count: u64) {
        self.gap_baseband += count as f64 * self.input_rate / self.device_rate;
        let baseband = self.gap_baseband as u64;
        self.gap_baseband -= baseband as f64;
        self.baseband_pos = self.baseband_pos.saturating_add(baseband);
        self.gap_audio += count as f64 * f64::from(AUDIO_RATE) / self.device_rate;
        let audio = self.gap_audio as u64;
        self.gap_audio -= audio as f64;
        self.sinks.pcm_pos.fetch_add(audio, Ordering::Relaxed);
        if self.produces_video {
            self.sinks.video_pos.fetch_add(baseband, Ordering::Relaxed);
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], center_hz: f64, lo_offset_hz: f64) {
        for block in input.chunks(DSP_BLOCK) {
            self.process_block(block, center_hz, lo_offset_hz);
        }
    }

    #[cfg(test)]
    fn process_and_flush(&mut self, input: &[Complex<f32>], center_hz: f64, lo_offset_hz: f64) {
        for block in input.chunks(DSP_BLOCK) {
            self.process(block, center_hz, lo_offset_hz);
            self.publisher.queue.flush();
        }
    }

    fn process_block(&mut self, input: &[Complex<f32>], center_hz: f64, lo_offset_hz: f64) {
        self.follow_lo_artifact(lo_offset_hz);
        self.ddc.process(input, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
        }
        if self.has_audio {
            self.audio.process_iq(&mut self.scratch);
        }
        self.filter.process(&self.scratch, &mut self.filtered);
        self.meter.process(&self.filtered);
        self.sinks
            .level_db
            .store(self.meter.level_db().to_bits(), Ordering::Relaxed);
        self.sinks
            .peak_db
            .store(self.meter.peak_db().to_bits(), Ordering::Relaxed);
        let baseband_start = self.baseband_pos;
        self.sink_baseband();
        let open = match self.threshold_db {
            Some(_) => self.squelch.process(&self.filtered),
            None => true,
        };
        self.sinks.squelch_db.store(
            match self.threshold_db {
                Some(_) => self.squelch.threshold_db().to_bits(),
                None => f32::NAN.to_bits(),
            },
            Ordering::Relaxed,
        );
        let video_pos = if self.produces_video {
            self.sinks
                .video_pos
                .fetch_add(self.filtered.len() as u64, Ordering::Relaxed)
                + self.filtered.len() as u64
        } else {
            0
        };
        self.outputs.reset();
        let wanted = self.publisher.wanted();
        self.outputs
            .symbols
            .set_wanted(wanted & SYMBOLS_WANTED != 0);
        let mut silence = 0;
        if open {
            self.rx.process(&self.filtered, &mut self.outputs);
            if self.has_audio {
                self.audio.process_audio(&mut self.outputs.audio_pcm);
            }
        } else {
            if self.emits_events {
                self.gated.clear();
                self.gated
                    .resize(self.filtered.len(), Complex::new(0.0, 0.0));
                self.rx.process(&self.gated, &mut self.outputs);
            }
            self.zero_carry += self.filtered.len() as f64 * self.pcm_per_input;
            self.outputs.audio_pcm.clear();
            silence = self.zero_carry as usize;
            self.zero_carry -= silence as f64;
        }
        let frames = if self.outputs.audio_pcm.is_empty() {
            silence
        } else {
            self.outputs.audio_pcm.len() / usize::from(self.audio_channels)
        };
        let audio_start = self
            .sinks
            .pcm_pos
            .fetch_add(frames as u64, Ordering::Relaxed);
        let published = self.publisher.queue.submit(|packet| {
            std::mem::swap(&mut packet.outputs, &mut self.outputs);
            packet.iq_wanted = wanted & IQ_WANTED != 0;
            packet.symbols_wanted = wanted & SYMBOLS_WANTED != 0;
            packet.iq_start = baseband_start;
            packet.input_len = self.filtered.len();
            if packet.iq_wanted || self.baseband_rec.is_some() {
                packet.iq.extend_from_slice(&self.filtered);
            }
            packet.audio_start = audio_start;
            packet.silence = silence;
            packet.channels = self.audio_channels;
            packet.frequency = center_hz + self.offset_hz;
            packet.video_position = video_pos;
            packet.recorder = self.audio_rec.clone();
            packet.baseband_recorder = self.baseband_rec.clone();
        });
        if !published {
            self.decoded.dropped.fetch_add(1, Ordering::Relaxed);
            if let Some(tap) = self.baseband_rec.take() {
                tap.publication_failed();
            }
            if let Some(tap) = self.audio_rec.take() {
                tap.publication_failed();
            }
        }
    }

    fn sink_baseband(&mut self) {
        self.baseband_pos += self.filtered.len() as u64;
        if self.baseband_rec.is_none() && self.baseband_export.is_none() {
            return;
        }
        if self.baseband_rec.as_ref().is_some_and(|tap| !tap.healthy()) {
            self.baseband_rec = None;
        }
        if self
            .baseband_export
            .as_mut()
            .is_some_and(|tap| !tap.push(&self.filtered))
        {
            self.baseband_export = None;
        }
    }

    fn follow_lo_artifact(&mut self, lo_offset_hz: f64) {
        let at = -(lo_offset_hz + self.offset_hz);
        let inside = at.abs() < self.input_rate / 2.0;
        let artifact = inside.then_some(at);
        if artifact != self.lo_artifact_hz {
            self.lo_artifact_hz = artifact;
            self.rx.lo_artifact_at(artifact);
        }
    }

    pub(crate) fn position_changed(&mut self, fix: Option<&PositionFix>) {
        self.position = fix.cloned();
        self.rx.position_changed(fix);
    }

    #[cfg(test)]
    pub(crate) fn retuned(&mut self) {
        self.audio.reset();
        self.squelch.reset();
    }

    pub(super) fn set_audio_recording(&mut self, tap: Option<AudioRecorderTap>) {
        self.audio_rec = tap;
    }

    pub(super) fn set_baseband_recording(&mut self, tap: Option<RecorderTap>) {
        self.baseband_rec = tap;
    }

    pub(super) fn set_baseband_export(&mut self, tap: Option<NetworkExportTap>) {
        self.baseband_export = tap;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use sdrmm_wire::{
        AudioAgcMode, AudioProcessing, ChannelParams, NfmParams, NoiseBlankerSettings, Sideband,
        SsbParams,
    };

    use super::*;
    use crate::audio::PcmPayload;

    const RATE: f64 = 48_000.0;
    const BLOCK: usize = 480;

    fn nfm_settings(squelch_db: Option<f32>) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db,
            squelch_auto_db: None,
            params: ChannelParams::Nfm(NfmParams::default()),
            audio: Default::default(),
        }
    }

    fn sinks(pcm_tx: broadcast::Sender<PcmBlock>, pcm_pos: Arc<AtomicU64>) -> ChannelSinks {
        ChannelSinks {
            publication: Arc::new(crate::metrics::QueueMetrics::default()),
            pcm_tx,
            pcm_pos,
            video_tx: broadcast::channel(8).0,
            video_pos: Arc::new(AtomicU64::new(0)),
            iq_tx: broadcast::channel(crate::iq::IQ_CHANNEL_CAP).0,
            symbol_tx: broadcast::channel(crate::symbols::SYMBOL_CHANNEL_CAP).0,
            level_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
            peak_db: Arc::new(AtomicU32::new(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits())),
            squelch_db: Arc::new(AtomicU32::new(f32::NAN.to_bits())),
        }
    }

    fn host(settings: &ChannelSettings) -> (Box<ChannelHost>, broadcast::Receiver<PcmBlock>) {
        let (pcm_tx, pcm_rx) = broadcast::channel(4096);
        let host = ChannelHost::build(
            RATE,
            settings,
            sinks(pcm_tx, Arc::new(AtomicU64::new(0))),
            DecodedSink::null(),
        )
        .expect("host builds");
        (host, pcm_rx)
    }

    fn tapped_host(
        settings: &ChannelSettings,
    ) -> (Box<ChannelHost>, broadcast::Receiver<crate::iq::IqBlock>) {
        let (pcm_tx, _pcm_rx) = broadcast::channel(4096);
        let mut built = sinks(pcm_tx, Arc::new(AtomicU64::new(0)));
        let (iq_tx, iq_rx) = broadcast::channel(64);
        built.iq_tx = iq_tx;
        let host =
            ChannelHost::build(RATE, settings, built, DecodedSink::null()).expect("host builds");
        (host, iq_rx)
    }

    fn tone(freq_hz: f64, amp: f32, len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                let p = TAU * freq_hz * k as f64 / RATE;
                Complex::new(p.cos() as f32 * amp, p.sin() as f32 * amp)
            })
            .collect()
    }

    fn run(
        host: &mut ChannelHost,
        rx: &mut broadcast::Receiver<PcmBlock>,
        input: &[Complex<f32>],
    ) -> Vec<PcmBlock> {
        let mut blocks = Vec::new();
        for chunk in input.chunks(BLOCK) {
            host.process_and_flush(chunk, 0.0, 0.0);
            while let Ok(block) = rx.try_recv() {
                blocks.push(block);
            }
        }
        blocks
    }

    fn goertzel_power(samples: &[f32], freq_hz: f64) -> f64 {
        let w = TAU * freq_hz / RATE;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &x in samples {
            let s0 = f64::from(x) + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        coeff.mul_add(-(s1 * s2), s1 * s1 + s2 * s2)
    }

    fn assert_silence_after_settle(blocks: &[PcmBlock], settle: u64, what: &str) {
        assert!(
            !blocks.is_empty(),
            "closed gate must still emit keep-alive fill"
        );
        let settled = blocks.iter().filter(|b| b.start_frame >= settle);
        let mut seen = 0usize;
        for block in settled {
            seen += 1;
            assert!(
                matches!(block.payload, PcmPayload::Silence(_)),
                "{what}: gate open on out-of-channel energy at stamp {}",
                block.start_frame
            );
        }
        assert!(seen > 0, "no settled blocks to judge");
    }

    #[test]
    fn a_gap_resets_the_receiver_and_advances_pcm_and_iq_timestamps() {
        let (mut used, mut received) = host(&nfm_settings(None));
        let first = tone(1200.0, 0.5, BLOCK);
        used.process_at(&first, 0, 100e6, 0.0);
        used.publisher.queue.flush();
        let before = received.try_recv().expect("first PCM");
        let before_len = match before.payload {
            PcmPayload::Samples(ref samples) => samples.len(),
            PcmPayload::Silence(n) => n,
        };
        let signal = tone(3400.0, 0.25, BLOCK);
        used.process_at(&signal, BLOCK as u64 + 4800, 100e6, 0.0);
        used.publisher.queue.flush();
        let after = received.try_recv().expect("post-gap PCM");
        assert_eq!(
            after.start_frame,
            before.start_frame + before_len as u64 + 4800
        );
        assert_eq!(used.baseband_pos, (BLOCK * 2 + 4800) as u64);
        let (mut fresh, mut fresh_pcm) = host(&nfm_settings(None));
        fresh.process_at(&signal, 0, 100e6, 0.0);
        fresh.publisher.queue.flush();
        let reference = fresh_pcm.try_recv().expect("fresh PCM");
        match (after.payload, reference.payload) {
            (PcmPayload::Samples(after), PcmPayload::Samples(reference)) => {
                assert_eq!(after, reference)
            }
            _ => panic!("expected decoded PCM"),
        }
    }

    #[test]
    fn adjacent_tone_does_not_open_the_squelch() {
        let (mut host, mut rx) = host(&nfm_settings(Some(-30.0)));
        let blocks = run(&mut host, &mut rx, &tone(15_000.0, 0.35, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "nfm");
    }

    #[test]
    fn ssb_squelch_gates_on_the_sideband_not_the_ddc_passband() {
        let settings = ChannelSettings {
            offset_hz: 0.0,
            squelch_db: Some(-50.0),
            squelch_auto_db: None,
            params: ChannelParams::Ssb(SsbParams {
                sideband: Sideband::Usb,
                bandwidth_hz: 2_700.0,
            }),
            audio: Default::default(),
        };
        let (mut host, mut rx) = host(&settings);
        let blocks = run(&mut host, &mut rx, &tone(10_000.0, 1.0, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "ssb");
    }

    #[test]
    fn an_automatic_squelch_finds_its_own_threshold() {
        let settings = ChannelSettings {
            squelch_db: Some(-100.0),
            squelch_auto_db: Some(8.0),
            ..nfm_settings(None)
        };
        let (mut host, mut rx) = host(&settings);
        let published = host.sinks.squelch_db.clone();

        let mut rng = 0x1234_5678u32;
        let mut noise = |len: usize| -> Vec<Complex<f32>> {
            (0..len)
                .map(|_| {
                    let mut next = || {
                        rng ^= rng << 13;
                        rng ^= rng >> 17;
                        rng ^= rng << 5;
                        (rng as f32 / u32::MAX as f32 - 0.5) * 0.002
                    };
                    Complex::new(next(), next())
                })
                .collect()
        };
        let quiet = run(&mut host, &mut rx, &noise(96_000));
        assert_silence_after_settle(&quiet, 24_000, "auto squelch on noise");
        let threshold = f32::from_bits(published.load(Ordering::Relaxed));
        assert!(
            (-100.0..-30.0).contains(&threshold),
            "threshold landed at {threshold} dB, nowhere near the noise it heard"
        );

        let loud = run(&mut host, &mut rx, &tone(0.0, 0.5, 48_000));
        assert!(
            loud.iter()
                .any(|b| matches!(b.payload, PcmPayload::Samples(_))),
            "the carrier never opened the gate"
        );
    }

    #[test]
    fn channel_filter_prevents_adjacent_capture() {
        let (mut host, mut rx) = host(&nfm_settings(None));
        let deviation = 2_500.0;
        let mut phase = 0.0f64;
        let wanted: Vec<Complex<f32>> = (0..96_000)
            .map(|k| {
                phase += TAU * deviation * (TAU * 1_000.0 * k as f64 / RATE).cos() / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect();
        let interferer = tone(15_000.0, 2.0, 96_000);
        let input: Vec<Complex<f32>> = wanted.iter().zip(&interferer).map(|(a, b)| a + b).collect();

        let mut audio: Vec<f32> = Vec::new();
        for chunk in input.chunks(BLOCK) {
            host.process_and_flush(chunk, 0.0, 0.0);
            while let Ok(block) = rx.try_recv() {
                if let PcmPayload::Samples(samples) = block.payload {
                    audio.extend_from_slice(&samples);
                }
            }
        }
        let window = &audio[48_000..96_000];
        let tone_power = goertzel_power(window, 1_000.0);
        let probes = [700.0, 1_500.0, 2_300.0].map(|f| goertzel_power(window, f));
        let mean = probes.iter().sum::<f64>() / probes.len() as f64;
        assert!(
            tone_power > 10.0 * mean,
            "adjacent capture: tone {tone_power:.3e} vs probe mean {mean:.3e}"
        );
    }

    #[test]
    fn pcm_stamps_are_contiguous_across_rebuild() {
        let (pcm_tx, mut rx) = broadcast::channel(4096);
        let pos = Arc::new(AtomicU64::new(0));
        let settings = nfm_settings(None);
        let mut host = ChannelHost::build(
            RATE,
            &settings,
            sinks(pcm_tx.clone(), pos.clone()),
            DecodedSink::null(),
        )
        .expect("host");
        let input = tone(1_000.0, 0.5, 24_000);
        for chunk in input.chunks(BLOCK) {
            host.process_and_flush(chunk, 0.0, 0.0);
        }
        let mut host = ChannelHost::build(RATE, &settings, sinks(pcm_tx, pos), DecodedSink::null())
            .expect("rebuilt host");
        for chunk in input.chunks(BLOCK) {
            host.process_and_flush(chunk, 0.0, 0.0);
        }

        let mut expected = 0u64;
        let mut seen = 0usize;
        while let Ok(block) = rx.try_recv() {
            assert_eq!(block.start_frame, expected, "stamp gap at block {seen}");
            let frames = match &block.payload {
                PcmPayload::Samples(s) => s.len() / usize::from(block.channels),
                PcmPayload::Silence(n) => *n,
            };
            expected += frames as u64;
            seen += 1;
        }
        assert_eq!(
            expected, 48_000,
            "both hosts' PCM must be stamped end to end"
        );
    }

    #[test]
    fn analog_media_publication_does_not_allocate_on_the_dsp_thread() {
        for kind in ["am", "nfm", "wfm", "ssb"] {
            let settings = ChannelSettings::default_for(kind).expect("registered analog channel");
            let rate = 240_000.0;
            let (pcm_tx, _pcm_rx) = broadcast::channel(128);
            let media = sinks(pcm_tx, Arc::new(AtomicU64::new(0)));
            let _iq_rx = media.iq_tx.subscribe();
            let mut host =
                ChannelHost::build(rate, &settings, media, DecodedSink::null()).expect("host");
            let input = vec![Complex::new(0.25, 0.1); DSP_BLOCK];
            for _ in 0..128 {
                host.process_and_flush(&input, 100e6, 0.0);
            }
            for _ in 0..128 {
                sdrmm_test_support::assert_no_alloc(kind, || host.process(&input, 100e6, 0.0));
                host.publisher.queue.flush();
            }
        }
    }

    #[test]
    fn the_receive_chain_runs_well_ahead_of_realtime_at_a_radios_rate() {
        const DEVICE_RATE: f64 = 2_400_000.0;
        const MTU: usize = 131_072;
        const SECONDS: f64 = 2.0;
        const MIN_FACTOR: f64 = 3.0;

        let settings = ChannelSettings {
            offset_hz: 250_000.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Wfm(sdrmm_wire::WfmParams::default()),
            audio: Default::default(),
        };
        let (pcm_tx, mut pcm_rx) = broadcast::channel::<PcmBlock>(4096);
        let mut host = ChannelHost::build(
            DEVICE_RATE,
            &settings,
            sinks(pcm_tx, Arc::new(AtomicU64::new(0))),
            DecodedSink::null(),
        )
        .expect("host builds");
        let block: Vec<Complex<f32>> = (0..MTU)
            .map(|k| {
                let p = TAU * 0.13 * k as f64;
                Complex::new(p.cos() as f32 * 0.4, p.sin() as f32 * 0.4)
            })
            .collect();

        let blocks = (DEVICE_RATE * SECONDS / MTU as f64) as usize;
        let start = std::time::Instant::now();
        for _ in 0..blocks {
            host.process(&block, 100_000_000.0, 0.0);
            while pcm_rx.try_recv().is_ok() {}
        }
        let factor = SECONDS / start.elapsed().as_secs_f64();
        assert!(
            factor > MIN_FACTOR,
            "wfm at {DEVICE_RATE} Sa/s ran at {factor:.1}x realtime, under the {MIN_FACTOR}x floor"
        );
    }

    #[test]
    fn the_baseband_tap_carries_the_channel_down_converted() {
        let mut settings = nfm_settings(None);
        settings.offset_hz = 3_000.0;
        let (mut host, mut rx) = tapped_host(&settings);

        let input = tone(3_000.0, 0.5, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process_and_flush(block, 100_000_000.0, 0.0);
        }

        let burst = rx.try_recv().expect("a subscribed tap sends bursts");
        assert_eq!(burst.samples.len(), crate::iq::IQ_BLOCK_SAMPLES);
        assert_eq!(
            burst.center_hz, 100_003_000.0,
            "the tap names the absolute centre"
        );
        assert_eq!(burst.sample_rate, RATE as f32);

        let tail = &burst.samples[burst.samples.len() / 2..];
        let mean: Complex<f32> = tail.iter().sum::<Complex<f32>>() / tail.len() as f32;
        let power: f32 = tail.iter().map(|s| s.norm()).sum::<f32>() / tail.len() as f32;
        assert!(
            mean.norm() > power * 0.9,
            "tap output is not at baseband: |mean| {:.4} against mean |s| {power:.4}",
            mean.norm()
        );
    }

    #[test]
    fn the_baseband_tap_keeps_running_through_a_closed_squelch() {
        let (mut host, mut rx) = tapped_host(&nfm_settings(Some(0.0)));

        let input = tone(0.0, 1e-6, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process_and_flush(block, 100_000_000.0, 0.0);
        }

        assert!(
            rx.try_recv().is_ok(),
            "the tap went quiet with the audio instead of showing the closed channel"
        );
    }

    #[test]
    fn an_unwatched_tap_sends_nothing() {
        let (mut host, _pcm) = host(&nfm_settings(None));
        let mut rx = host.sinks.iq_tx.subscribe();
        drop(rx);
        rx = host.sinks.iq_tx.subscribe();
        drop(rx);

        let input = tone(0.0, 0.5, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process_and_flush(block, 100_000_000.0, 0.0);
        }
        assert_eq!(host.sinks.iq_tx.receiver_count(), 0);
        assert!(host.sinks.iq_tx.subscribe().try_recv().is_err());
    }

    fn nfm_audio_settings(audio: AudioProcessing) -> ChannelSettings {
        ChannelSettings {
            audio,
            ..nfm_settings(None)
        }
    }

    fn fm_tone(f_mod: f64, deviation_hz: f64, len: usize) -> Vec<Complex<f32>> {
        let mut phase = 0.0f64;
        (0..len)
            .map(|k| {
                phase += TAU * deviation_hz * (TAU * f_mod * k as f64 / RATE).cos() / RATE;
                Complex::from_polar(1.0, phase as f32)
            })
            .collect()
    }

    fn samples(blocks: &[PcmBlock]) -> Vec<f32> {
        blocks
            .iter()
            .flat_map(|block| match &block.payload {
                PcmPayload::Samples(pcm) => pcm.to_vec(),
                PcmPayload::Silence(n) => vec![0.0; *n],
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt() as f32
    }

    #[test]
    fn the_audio_chain_runs_on_a_channel_that_never_had_one() {
        let quiet = fm_tone(1_000.0, 250.0, 192_000);
        let (mut plain, mut rx) = host(&nfm_settings(None));
        let untouched = samples(&run(&mut plain, &mut rx, &quiet));
        assert!(rms(&untouched[96_000..]) < 0.15, "signal was not quiet");

        let (mut levelled, mut rx) = host(&nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        }));
        let lifted = samples(&run(&mut levelled, &mut rx, &quiet));
        let level = rms(&lifted[144_000..]);
        assert!((0.18..0.32).contains(&level), "levelled to {level}");
    }

    #[test]
    fn a_default_chain_leaves_the_audio_alone() {
        let input = fm_tone(1_000.0, 2_500.0, 48_000);
        let (mut plain, mut rx) = host(&nfm_settings(None));
        let plain_audio = samples(&run(&mut plain, &mut rx, &input));
        let (mut chained, mut rx) = host(&nfm_audio_settings(AudioProcessing::default()));
        assert_eq!(samples(&run(&mut chained, &mut rx, &input)), plain_audio);
    }

    #[test]
    fn the_blanker_cleans_impulses_before_the_channel_filter() {
        let mut input = fm_tone(1_000.0, 2_500.0, 96_000);
        for n in (1_000..input.len()).step_by(1_000) {
            input[n] = Complex::new(6.0, 0.0);
        }

        let peak = |settings: &ChannelSettings| {
            let (mut host, mut rx) = tapped_host(settings);
            let mut worst = 0.0f32;
            for chunk in input.chunks(BLOCK) {
                host.process_and_flush(chunk, 0.0, 0.0);
                while let Ok(block) = rx.try_recv() {
                    if block.timestamp < 8_192 {
                        continue;
                    }
                    worst = block.samples.iter().fold(worst, |m, s| m.max(s.norm()));
                }
            }
            worst
        };

        let dirty = peak(&nfm_settings(None));
        assert!(dirty > 1.5, "the impulses never reached the demod: {dirty}");
        let clean = peak(&nfm_audio_settings(AudioProcessing {
            blanker: NoiseBlankerSettings {
                enabled: true,
                threshold: 4.0,
            },
            ..AudioProcessing::default()
        }));
        assert!(
            clean < 1.3 && clean < 0.6 * dirty,
            "impulses survived into the demod: {dirty} -> {clean}"
        );
    }

    #[test]
    fn prepared_replacement_switches_a_stage_and_keeps_recording_positions() {
        let (mut host, mut rx) = host(&nfm_settings(None));
        let quiet = fm_tone(1_000.0, 250.0, 96_000);
        run(&mut host, &mut rx, &quiet);
        let settings = nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        });
        let mut replacement =
            ChannelHost::build(RATE, &settings, host.sinks.clone(), DecodedSink::null())
                .expect("prepared host");
        replacement.inherit(&mut host);
        let before = host.sinks.pcm_pos.load(Ordering::Relaxed);
        host = replacement;
        let lifted = samples(&run(&mut host, &mut rx, &quiet));
        assert!(host.sinks.pcm_pos.load(Ordering::Relaxed) > before);
        let level = rms(&lifted[48_000..]);
        assert!((0.18..0.32).contains(&level), "levelled to {level}");
    }

    #[test]
    fn retuning_forgets_what_the_chain_learnt() {
        let (mut host, mut rx) = host(&nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Slow,
            ..AudioProcessing::default()
        }));
        run(&mut host, &mut rx, &fm_tone(1_000.0, 250.0, 96_000));
        host.retuned();
        let after = samples(&run(&mut host, &mut rx, &fm_tone(1_000.0, 2_500.0, 4_800)));
        assert!(rms(&after[..2_400]) < 1.0, "the old gain followed the tune");
    }
}
