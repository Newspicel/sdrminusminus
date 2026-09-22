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
use sdrmm_dsp::{LevelMeter, Squelch};
use sdrmm_wire::{ChannelSettings, DecoderEvent, PositionFix};
use tokio::sync::broadcast;

use super::{downconvert::Downconverter, dsp_block_len};
use crate::{
    Doppler,
    audio::PcmBlock,
    audio_recording::AudioRecorderTap,
    iq::IqBlock,
    network_export::NetworkExportTap,
    publishing::channel::{ChannelPublisher, IQ_WANTED, SYMBOLS_WANTED},
    recording::RecorderTap,
    symbols::SymbolBlock,
    video::VideoPacket,
};

const SQUELCH_HYSTERESIS_DB: f32 = 2.0;
const SQUELCH_HOLD_S: f32 = 0.1;
const DOPPLER_RAMP_S: f64 = 2.0;

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

    pub(crate) fn device_set(&self) -> u32 {
        self.device_set
    }

    pub(crate) fn channel(&self) -> u32 {
        self.channel
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
    ddc: Downconverter,
    filter: ChannelFilter,
    squelch: Squelch,
    squelched: bool,
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
    frequency_hz: f64,
    offset_hz: f64,
    center_hz: f64,
    band_low_hz: f64,
    band_high_hz: f64,
    in_band: bool,
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
    doppler: Doppler,
    ramp_left: u64,
}

/// Whether the radio's window covers everything this channel occupies.
pub(crate) fn reaches(offset_hz: f64, low_hz: f64, high_hz: f64, device_rate: f64) -> bool {
    let nyquist = device_rate / 2.0;
    offset_hz + low_hz >= -nyquist && offset_hz + high_hz <= nyquist
}

impl ChannelHost {
    pub(crate) fn build(
        device_rate: f64,
        center_hz: f64,
        settings: &ChannelSettings,
        sinks: ChannelSinks,
        decoded: DecodedSink,
    ) -> Result<Box<Self>, ChannelError> {
        let type_id = settings.params.type_id();
        let descriptor = sdrmm_channels::descriptor(type_id)
            .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
        let input_rate = sdrmm_channels::input_rate(&settings.params);
        let offset_hz = settings.frequency_hz - center_hz;
        let (band_low_hz, band_high_hz) = sdrmm_channels::occupied_band(&settings.params);
        let ddc = Downconverter::new(device_rate, input_rate, offset_hz)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        let filter = sdrmm_channels::channel_filter(&settings.params)?;
        let rx = sdrmm_channels::create(ChannelCtx { input_rate }, settings)?;
        let audio_channels = sdrmm_channels::audio_channels(&settings.params);
        let decodes = descriptor.decoder_kind.is_some();
        let emits_events = decodes && rx.needs_gated_input();
        let mut squelch = Squelch::new(
            input_rate,
            settings.squelch.manual_level_db().unwrap_or(0.0),
            SQUELCH_HYSTERESIS_DB,
            SQUELCH_HOLD_S,
        );
        squelch.set_auto_margin_db(settings.squelch.auto_margin_db());
        let publisher =
            ChannelPublisher::new(input_rate, device_rate, settings, sinks.clone(), decoded)
                .map_err(|error| {
                    ChannelError::InvalidSettings(format!("start publisher: {error}"))
                })?;
        Ok(Box::new(Self {
            ddc,
            filter,
            squelch,
            squelched: !settings.squelch.is_off(),
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
            frequency_hz: settings.frequency_hz,
            offset_hz,
            center_hz,
            band_low_hz,
            band_high_hz,
            in_band: reaches(offset_hz, band_low_hz, band_high_hz, device_rate),
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
            doppler: Doppler::default(),
            ramp_left: 0,
        }))
    }

    pub(super) fn inherit(&mut self, previous: &mut Self) {
        self.follow(previous);
        self.audio_rec = previous.audio_rec.take();
        self.baseband_rec = previous.baseband_rec.take();
        self.baseband_export = previous.baseband_export.take();
        self.baseband_pos = previous.baseband_pos;
        self.gap_audio = previous.gap_audio;
        self.gap_baseband = previous.gap_baseband;
        self.doppler = previous.doppler;
        self.ramp_left = previous.ramp_left;
        self.place();
    }

    pub(super) fn follow(&mut self, previous: &Self) {
        self.publisher.queue.follow(&previous.publisher.queue);
        self.next_input = previous.next_input;
    }

    pub(super) fn process_at(&mut self, input: &[Complex<f32>], index: u64, center_hz: f64) {
        self.process_shared_at(input, index, center_hz, None);
    }

    pub(super) fn subband(&mut self, center_hz: f64, sample_rate: f64) -> Option<usize> {
        self.follow_center(center_hz);
        (self.in_band && self.device_rate == sample_rate)
            .then(|| self.ddc.band())
            .flatten()
    }

    pub(super) fn process_shared_at(
        &mut self,
        input: &[Complex<f32>],
        index: u64,
        center_hz: f64,
        selected: Option<&[Complex<f32>]>,
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
        if selected.is_some() {
            self.process_selected_block(input, center_hz, selected);
        } else {
            self.process(input, center_hz);
        }
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

    pub(super) fn process(&mut self, input: &[Complex<f32>], center_hz: f64) {
        for block in input.chunks(dsp_block_len(self.device_rate)) {
            self.process_block(block, center_hz);
        }
    }

    #[cfg(test)]
    fn process_and_flush(&mut self, input: &[Complex<f32>], center_hz: f64) {
        for block in input.chunks(dsp_block_len(self.device_rate)) {
            self.process(block, center_hz);
            self.publisher.queue.flush();
        }
    }

    fn process_block(&mut self, input: &[Complex<f32>], center_hz: f64) {
        self.process_selected_block(input, center_hz, None);
    }

    fn process_selected_block(
        &mut self,
        input: &[Complex<f32>],
        center_hz: f64,
        selected: Option<&[Complex<f32>]>,
    ) {
        self.ramp_doppler(input.len());
        self.follow_center(center_hz);
        if !self.in_band {
            self.sinks
                .level_db
                .store(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits(), Ordering::Relaxed);
            self.sinks
                .peak_db
                .store(sdrmm_dsp::LEVEL_FLOOR_DB.to_bits(), Ordering::Relaxed);
            self.sinks
                .squelch_db
                .store(f32::NAN.to_bits(), Ordering::Relaxed);
            self.skip_input(input.len() as u64);
            return;
        }
        self.follow_lo_artifact();
        if self.ddc.select_shared(selected.is_some()) {
            self.filter.reset();
            self.audio.reset();
            self.squelch.reset();
        }
        self.ddc.process(input, selected, &mut self.scratch);
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
        let open = !self.squelched || self.squelch.process(&self.filtered);
        self.sinks.squelch_db.store(
            if self.squelched {
                self.squelch.threshold_db().to_bits()
            } else {
                f32::NAN.to_bits()
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
            packet.frequency = self.frequency_hz;
            packet.video_position = video_pos;
            packet.recorder = self.audio_rec.clone();
            packet.baseband_recorder = self.baseband_rec.clone();
        });
        if !published {
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

    /// Keeps the decoder on its own frequency while the radio moves under it, and mutes it for as
    /// long as the radio is tuned somewhere it cannot hear.
    fn follow_center(&mut self, center_hz: f64) {
        if center_hz == self.center_hz {
            return;
        }
        self.center_hz = center_hz;
        self.place();
    }

    /// Moves the decoder to another frequency without rebuilding it, as a scan stepping through
    /// its targets does.
    pub(crate) fn retune(&mut self, frequency_hz: f64) {
        if frequency_hz == self.frequency_hz {
            return;
        }
        self.frequency_hz = frequency_hz;
        self.place();
    }

    fn place(&mut self) {
        if self.aim() {
            self.restart_chain();
        }
    }

    pub(crate) fn steer(&mut self, doppler: Doppler) {
        self.doppler = doppler;
        self.ramp_left = (DOPPLER_RAMP_S * self.device_rate) as u64;
        self.glide();
    }

    fn ramp_doppler(&mut self, samples: usize) {
        if self.ramp_left == 0 || self.doppler.rate_hz_s == 0.0 {
            return;
        }
        let ramped = (samples as u64).min(self.ramp_left);
        self.ramp_left -= ramped;
        self.doppler.shift_hz += self.doppler.rate_hz_s * ramped as f64 / self.device_rate;
        self.glide();
    }

    fn glide(&mut self) {
        let was_in_band = self.in_band;
        if self.aim() && !was_in_band {
            self.restart_chain();
        }
    }

    fn aim(&mut self) -> bool {
        let offset_hz = self.frequency_hz + self.doppler.shift_hz - self.center_hz;
        self.in_band = reaches(
            offset_hz,
            self.band_low_hz,
            self.band_high_hz,
            self.device_rate,
        );
        if !self.in_band || offset_hz == self.offset_hz {
            return false;
        }
        self.offset_hz = offset_hz;
        self.ddc.set_offset(offset_hz);
        true
    }

    fn restart_chain(&mut self) {
        self.ddc.reset();
        self.filter.reset();
        self.audio.reset();
        self.squelch.reset();
        self.lo_artifact_hz = None;
    }

    fn follow_lo_artifact(&mut self) {
        let at = -self.offset_hz;
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
        AmParams, AudioAgcMode, AudioProcessing, ChannelParams, NfmParams, NoiseBlankerSettings,
        Sideband, SsbParams, WfmParams,
    };

    use super::{super::DSP_BLOCK, *};
    use crate::audio::PcmPayload;

    const RATE: f64 = 48_000.0;
    const BLOCK: usize = 480;
    const CENTER: f64 = 100_000_000.0;

    fn nfm_settings(squelch: sdrmm_wire::Squelch) -> ChannelSettings {
        ChannelSettings {
            frequency_hz: CENTER,
            squelch,
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
            CENTER,
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
        let host = ChannelHost::build(RATE, CENTER, settings, built, DecodedSink::null())
            .expect("host builds");
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
            host.process_and_flush(chunk, CENTER);
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
    fn a_retune_moves_the_decoder_and_a_step_off_the_window_mutes_it() {
        let (mut host, _rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        assert!(host.in_band);
        host.retune(CENTER + 10_000.0);
        assert_eq!(host.offset_hz, 10_000.0);
        assert!(
            host.in_band,
            "10 kHz off centre sits inside a 48 kHz window"
        );

        host.retune(CENTER + 40_000.0);
        assert!(!host.in_band, "40 kHz off centre falls outside the window");
        assert_eq!(
            host.offset_hz, 10_000.0,
            "an unreachable offset is not mixed to"
        );

        host.process_and_flush(&tone(1200.0, 0.5, BLOCK), CENTER + 40_000.0);
        assert!(
            host.in_band,
            "the radio moving over the decoder brings it back"
        );
        assert_eq!(host.offset_hz, 0.0);
    }

    #[test]
    fn a_gap_resets_the_receiver_and_advances_pcm_and_iq_timestamps() {
        let (mut used, mut received) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        let first = tone(1200.0, 0.5, BLOCK);
        used.process_at(&first, 0, 100e6);
        used.publisher.queue.flush();
        let before = received.try_recv().expect("first PCM");
        let before_len = match before.payload {
            PcmPayload::Samples(ref samples) => samples.len(),
            PcmPayload::Silence(n) => n,
        };
        let signal = tone(3400.0, 0.25, BLOCK);
        used.process_at(&signal, BLOCK as u64 + 4800, 100e6);
        used.publisher.queue.flush();
        let after = received.try_recv().expect("post-gap PCM");
        assert_eq!(
            after.start_frame,
            before.start_frame + before_len as u64 + 4800
        );
        assert_eq!(used.baseband_pos, (BLOCK * 2 + 4800) as u64);
        let (mut fresh, mut fresh_pcm) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        fresh.process_at(&signal, 0, 100e6);
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
    fn shared_bands_keep_pcm_continuous_across_membership_and_tuning_changes() {
        use super::super::subbands::Subbands;

        let rate = 20_000_000.0;
        let mut settings = nfm_settings(sdrmm_wire::Squelch::Off);
        settings.frequency_hz = CENTER + 100_000.0;
        let (pcm_tx, mut pcm_rx) = broadcast::channel(4096);
        let mut channels = vec![(
            1,
            ChannelHost::build(
                rate,
                CENTER,
                &settings,
                sinks(pcm_tx, Arc::new(AtomicU64::new(0))),
                DecodedSink::null(),
            )
            .unwrap(),
        )];
        let mut bank = Subbands::new(rate);
        let input = vec![Complex::new(0.5, 0.25); dsp_block_len(rate)];
        let mut index = 0;
        let mut expected = 0;
        for (stage, count) in [1, 2, 7, 8, 7, 8, 13, 2, 1].into_iter().enumerate() {
            channels.truncate(count);
            while channels.len() < count {
                let id = channels.len() as u32 + 1;
                let (pcm_tx, _) = broadcast::channel(8);
                channels.push((
                    id,
                    ChannelHost::build(
                        rate,
                        CENTER,
                        &settings,
                        sinks(pcm_tx, Arc::new(AtomicU64::new(0))),
                        DecodedSink::null(),
                    )
                    .unwrap(),
                ));
            }
            let center = CENTER + stage as f64 * 1000.0;
            let offset = if stage >= 2 { 1_700_000.0 } else { 100_000.0 };
            let plan = sdrmm_dsp::subband::SubbandPlan::new(rate).unwrap();
            let target = plan.select(offset, 48_000.0).unwrap();
            for (index, (_, host)) in channels.iter_mut().enumerate() {
                let offset = if count >= 7 && index > 0 {
                    let band = index - 1 + usize::from(index > target);
                    plan.center(band) + 100_000.0
                } else {
                    offset
                };
                host.retune(center + offset);
            }
            for _ in 0..100 {
                bank.prepare(&mut channels, center, rate);
                bank.process(&input, index);
                for (_, host) in &mut channels {
                    let selected = host
                        .subband(center, rate)
                        .and_then(|band| bank.samples(band));
                    assert_eq!(selected.is_some(), count == 2 || count >= 8);
                    host.process_shared_at(&input, index, center, selected);
                    host.publisher.queue.flush();
                }
                index += input.len() as u64;
                while let Ok(block) = pcm_rx.try_recv() {
                    assert_eq!(block.start_frame, expected);
                    let PcmPayload::Samples(samples) = block.payload else {
                        panic!("PCM");
                    };
                    assert!(samples.iter().all(|sample| sample.is_finite()));
                    expected += samples.len() as u64;
                }
            }
        }
        let nominal = index as f64 * 48_000.0 / rate;
        assert!(
            (expected as f64 - nominal).abs() < 16.0,
            "frames={expected} nominal={nominal}"
        );
        index += 200_000;
        let center = CENTER + 3000.0;
        bank.process(&input, index);
        let host = &mut channels[0].1;
        let selected = host
            .subband(center, rate)
            .and_then(|band| bank.samples(band));
        host.process_shared_at(&input, index, center, selected);
        host.publisher.queue.flush();
        assert_eq!(pcm_rx.try_recv().unwrap().start_frame, expected + 480);
    }

    #[test]
    fn adjacent_tone_does_not_open_the_squelch() {
        let (mut host, mut rx) = host(&nfm_settings(sdrmm_wire::Squelch::Manual {
            level_db: -30.0,
        }));
        let blocks = run(&mut host, &mut rx, &tone(15_000.0, 0.35, 48_000));
        assert_silence_after_settle(&blocks, 24_000, "nfm");
    }

    #[test]
    fn ssb_squelch_gates_on_the_sideband_not_the_ddc_passband() {
        let settings = ChannelSettings {
            frequency_hz: CENTER,
            squelch: sdrmm_wire::Squelch::Manual { level_db: -50.0 },
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

    fn noise(len: usize) -> Vec<Complex<f32>> {
        let mut rng = 0x1234_5678u32;
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
    }

    fn am_settings(squelch: sdrmm_wire::Squelch) -> ChannelSettings {
        ChannelSettings {
            frequency_hz: CENTER,
            squelch,
            params: ChannelParams::Am(AmParams::default()),
            audio: Default::default(),
        }
    }

    #[test]
    fn an_am_gate_set_just_above_the_floor_closes_again_after_a_burst() {
        let (mut probe, mut probe_rx) = host(&am_settings(sdrmm_wire::Squelch::Off));
        let _ = run(&mut probe, &mut probe_rx, &noise(96_000));
        let floor = f32::from_bits(probe.sinks.level_db.load(Ordering::Relaxed));

        let (mut host, mut rx) = host(&am_settings(sdrmm_wire::Squelch::Manual {
            level_db: floor + 3.0,
        }));
        let quiet = run(&mut host, &mut rx, &noise(48_000));
        assert_silence_after_settle(&quiet, 24_000, "am floor");

        let loud = run(&mut host, &mut rx, &tone(0.0, 0.5, 48_000));
        assert!(
            loud.iter()
                .any(|b| matches!(b.payload, PcmPayload::Samples(_))),
            "the carrier never opened the gate"
        );

        let settle = host.sinks.pcm_pos.load(Ordering::Relaxed) + 24_000;
        let after = run(&mut host, &mut rx, &noise(96_000));
        assert_silence_after_settle(&after, settle, "am floor after a burst");
    }

    #[test]
    fn an_automatic_squelch_finds_its_own_threshold() {
        let settings = nfm_settings(sdrmm_wire::Squelch::Auto { margin_db: 8.0 });
        let (mut host, mut rx) = host(&settings);
        let published = host.sinks.squelch_db.clone();

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
        let (mut host, mut rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
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
            host.process_and_flush(chunk, CENTER);
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
        let settings = nfm_settings(sdrmm_wire::Squelch::Off);
        let mut host = ChannelHost::build(
            RATE,
            CENTER,
            &settings,
            sinks(pcm_tx.clone(), pos.clone()),
            DecodedSink::null(),
        )
        .expect("host");
        let input = tone(1_000.0, 0.5, 24_000);
        for chunk in input.chunks(BLOCK) {
            host.process_and_flush(chunk, CENTER);
        }
        let mut host = ChannelHost::build(
            RATE,
            CENTER,
            &settings,
            sinks(pcm_tx, pos),
            DecodedSink::null(),
        )
        .expect("rebuilt host");
        for chunk in input.chunks(BLOCK) {
            host.process_and_flush(chunk, CENTER);
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
    fn pcm_stamps_are_contiguous_across_a_layout_change() {
        const WFM_RATE: f64 = 240_000.0;
        let (pcm_tx, mut rx) = broadcast::channel(4096);
        let pos = Arc::new(AtomicU64::new(0));
        let wfm = |stereo: bool| ChannelSettings {
            frequency_hz: CENTER,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Wfm(WfmParams {
                deemphasis_us: 50.0,
                stereo,
            }),
            audio: Default::default(),
        };
        let input = tone(1_000.0, 0.5, 48_000);
        let mut expected = 0u64;
        let mut layouts = Vec::new();
        for stereo in [true, false] {
            let mut host = ChannelHost::build(
                WFM_RATE,
                CENTER,
                &wfm(stereo),
                sinks(pcm_tx.clone(), pos.clone()),
                DecodedSink::null(),
            )
            .expect("host");
            for chunk in input.chunks(BLOCK) {
                host.process_and_flush(chunk, CENTER);
            }
            while let Ok(block) = rx.try_recv() {
                assert_eq!(
                    block.start_frame, expected,
                    "stamp gap while stereo is {stereo}"
                );
                let frames = match &block.payload {
                    PcmPayload::Samples(s) => s.len() / usize::from(block.channels),
                    PcmPayload::Silence(n) => *n,
                };
                expected += frames as u64;
                layouts.push(block.channels);
            }
        }
        assert_eq!(layouts.first(), Some(&2), "the stream did not start stereo");
        assert_eq!(layouts.last(), Some(&1), "the layout never followed");
        assert!(expected > 0, "no PCM to judge");
    }

    #[test]
    fn a_decoder_change_orders_queued_pcm_without_inheriting_iq_positions() {
        let (pcm_tx, mut received) = broadcast::channel(4096);
        let pos = Arc::new(AtomicU64::new(0));
        let mut settings = nfm_settings(sdrmm_wire::Squelch::Off);
        let mut previous = ChannelHost::build(
            RATE,
            CENTER,
            &settings,
            sinks(pcm_tx.clone(), pos.clone()),
            DecodedSink::null(),
        )
        .unwrap();
        let input = tone(1000.0, 0.5, BLOCK);
        for index in 0..5 {
            previous.process_at(&input, (index * BLOCK) as u64, CENTER);
        }
        assert_eq!(previous.baseband_pos, (5 * BLOCK) as u64);
        settings.params = ChannelParams::Am(Default::default());
        let mut next = ChannelHost::build(
            RATE,
            CENTER,
            &settings,
            sinks(pcm_tx, pos),
            DecodedSink::null(),
        )
        .unwrap();
        next.follow(&previous);
        assert_eq!(next.baseband_pos, 0);
        let retirement = std::thread::spawn(move || drop(previous));
        for index in 5..10 {
            next.process_at(&input, (index * BLOCK) as u64, CENTER);
        }
        retirement.join().unwrap();
        next.publisher.queue.flush();
        let mut expected = 0;
        while let Ok(block) = received.try_recv() {
            assert_eq!(block.start_frame, expected);
            let PcmPayload::Samples(samples) = block.payload else {
                panic!("PCM");
            };
            expected += samples.len() as u64 / u64::from(block.channels);
        }
        assert_eq!(expected, (10 * BLOCK) as u64);
    }

    #[test]
    fn analog_media_publication_does_not_allocate_on_the_dsp_thread() {
        for rate in [240_000.0, 8_000_000.0, 20_000_000.0] {
            for kind in ["am", "nfm", "wfm", "ssb"] {
                let settings =
                    ChannelSettings::default_for(kind).expect("registered analog channel");
                let (pcm_tx, _pcm_rx) = broadcast::channel(128);
                let media = sinks(pcm_tx, Arc::new(AtomicU64::new(0)));
                let _iq_rx = media.iq_tx.subscribe();
                let mut host =
                    ChannelHost::build(rate, CENTER, &settings, media, DecodedSink::null())
                        .expect("host");
                let input = vec![Complex::new(0.25, 0.1); dsp_block_len(rate)];
                for _ in 0..128 {
                    host.process_and_flush(&input, CENTER);
                }
                for _ in 0..128 {
                    sdrmm_test_support::assert_no_alloc(kind, || host.process(&input, CENTER));
                    host.publisher.queue.flush();
                }
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
            frequency_hz: CENTER + 250_000.0,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Wfm(sdrmm_wire::WfmParams::default()),
            audio: Default::default(),
        };
        let (pcm_tx, mut pcm_rx) = broadcast::channel::<PcmBlock>(4096);
        let mut host = ChannelHost::build(
            DEVICE_RATE,
            CENTER,
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
            host.process(&block, CENTER);
            while pcm_rx.try_recv().is_ok() {}
        }
        let factor = SECONDS / start.elapsed().as_secs_f64();
        assert!(
            factor > MIN_FACTOR,
            "wfm at {DEVICE_RATE} S/s ran at {factor:.1}x realtime, under the {MIN_FACTOR}x floor"
        );
    }

    #[test]
    fn a_decoder_holds_its_frequency_while_the_radio_moves_under_it() {
        const DEVICE_RATE: f64 = 240_000.0;
        let mut settings = nfm_settings(sdrmm_wire::Squelch::Off);
        settings.frequency_hz = CENTER + 50_000.0;
        let (pcm_tx, _pcm_rx) = broadcast::channel(4096);
        let mut built = sinks(pcm_tx, Arc::new(AtomicU64::new(0)));
        let (iq_tx, mut iq_rx) = broadcast::channel(64);
        built.iq_tx = iq_tx;
        let mut host =
            ChannelHost::build(DEVICE_RATE, CENTER, &settings, built, DecodedSink::null())
                .expect("host builds");

        let carrier = |offset_hz: f64| -> Vec<Complex<f32>> {
            (0..crate::iq::IQ_BLOCK_SAMPLES * 40)
                .map(|k| {
                    let p = TAU * offset_hz * k as f64 / DEVICE_RATE;
                    Complex::new(p.cos() as f32 * 0.5, p.sin() as f32 * 0.5)
                })
                .collect()
        };
        let run = |host: &mut ChannelHost, input: &[Complex<f32>], center_hz: f64| {
            for block in input.chunks(DSP_BLOCK) {
                host.process_and_flush(block, center_hz);
            }
        };

        run(&mut host, &carrier(50_000.0), CENTER);
        assert_eq!(
            drain(&mut iq_rx).last().map(|burst| burst.center_hz),
            Some(CENTER + 50_000.0),
            "the tap did not name the frequency the decoder was set to"
        );

        run(&mut host, &carrier(25_000.0), CENTER + 25_000.0);
        assert_eq!(
            drain(&mut iq_rx).last().map(|burst| burst.center_hz),
            Some(CENTER + 50_000.0),
            "the decoder drifted with the radio instead of holding its frequency"
        );

        run(&mut host, &carrier(0.0), CENTER + 5_000_000.0);
        assert!(
            drain(&mut iq_rx).is_empty(),
            "a radio tuned right off the decoder still produced baseband"
        );

        run(&mut host, &carrier(50_000.0), CENTER);
        assert_eq!(
            drain(&mut iq_rx).last().map(|burst| burst.center_hz),
            Some(CENTER + 50_000.0),
            "the decoder did not come back when the radio returned over it"
        );
    }

    fn drain(rx: &mut broadcast::Receiver<crate::iq::IqBlock>) -> Vec<crate::iq::IqBlock> {
        let mut bursts = Vec::new();
        while let Ok(burst) = rx.try_recv() {
            bursts.push(burst);
        }
        bursts
    }

    #[test]
    fn the_baseband_tap_carries_the_channel_down_converted() {
        let mut settings = nfm_settings(sdrmm_wire::Squelch::Off);
        settings.frequency_hz = CENTER + 3_000.0;
        let (mut host, mut rx) = tapped_host(&settings);

        let input = tone(3_000.0, 0.5, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process_and_flush(block, CENTER);
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
        let (mut host, mut rx) =
            tapped_host(&nfm_settings(sdrmm_wire::Squelch::Manual { level_db: 0.0 }));

        let input = tone(0.0, 1e-6, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process_and_flush(block, CENTER);
        }

        assert!(
            rx.try_recv().is_ok(),
            "the tap went quiet with the audio instead of showing the closed channel"
        );
    }

    #[test]
    fn an_unwatched_tap_sends_nothing() {
        let (mut host, _pcm) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        let mut rx = host.sinks.iq_tx.subscribe();
        drop(rx);
        rx = host.sinks.iq_tx.subscribe();
        drop(rx);

        let input = tone(0.0, 0.5, crate::iq::IQ_BLOCK_SAMPLES * 4);
        for block in input.chunks(BLOCK) {
            host.process_and_flush(block, CENTER);
        }
        assert_eq!(host.sinks.iq_tx.receiver_count(), 0);
        assert!(host.sinks.iq_tx.subscribe().try_recv().is_err());
    }

    fn nfm_audio_settings(audio: AudioProcessing) -> ChannelSettings {
        ChannelSettings {
            audio,
            ..nfm_settings(sdrmm_wire::Squelch::Off)
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
        let (mut plain, mut rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
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
        let (mut plain, mut rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
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
                host.process_and_flush(chunk, CENTER);
                while let Ok(block) = rx.try_recv() {
                    if block.timestamp < 8_192 {
                        continue;
                    }
                    worst = block.samples.iter().fold(worst, |m, s| m.max(s.norm()));
                }
            }
            worst
        };

        let dirty = peak(&nfm_settings(sdrmm_wire::Squelch::Off));
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
        let (mut host, mut rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        let quiet = fm_tone(1_000.0, 250.0, 96_000);
        run(&mut host, &mut rx, &quiet);
        let settings = nfm_audio_settings(AudioProcessing {
            agc: AudioAgcMode::Fast,
            ..AudioProcessing::default()
        });
        let mut replacement = ChannelHost::build(
            RATE,
            CENTER,
            &settings,
            host.sinks.clone(),
            DecodedSink::null(),
        )
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

    #[test]
    fn a_doppler_steer_follows_the_carrier_without_moving_the_decoder() {
        let (mut host, _rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        host.steer(Doppler {
            shift_hz: 3_000.0,
            rate_hz_s: 0.0,
        });
        assert_eq!(host.offset_hz, 3_000.0);
        assert_eq!(
            host.frequency_hz, CENTER,
            "the tuned frequency stays the published one"
        );
    }

    #[test]
    fn a_doppler_ramp_glides_between_updates_and_stops_when_they_do() {
        let (mut host, _rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        host.steer(Doppler {
            shift_hz: 0.0,
            rate_hz_s: -100.0,
        });
        let second = RATE as usize;
        host.process_and_flush(&tone(0.0, 0.1, second), CENTER);
        assert!((host.offset_hz + 100.0).abs() < 1.0, "{}", host.offset_hz);
        host.process_and_flush(&tone(0.0, 0.1, 3 * second), CENTER);
        assert!(
            (host.offset_hz + 100.0 * DOPPLER_RAMP_S).abs() < 1.0,
            "a silent tracker must not steer forever: {}",
            host.offset_hz
        );
    }

    #[test]
    fn a_doppler_steer_off_the_window_mutes_and_back_on_restores() {
        let (mut host, _rx) = host(&nfm_settings(sdrmm_wire::Squelch::Off));
        host.steer(Doppler {
            shift_hz: 40_000.0,
            rate_hz_s: 0.0,
        });
        assert!(!host.in_band);
        host.steer(Doppler::default());
        assert!(host.in_band);
        assert_eq!(host.offset_hz, 0.0);
    }
}
