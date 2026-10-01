use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Soft, design_lowpass};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, BroadcastStatus, BroadcastSystem, ChannelDescriptor,
    ChannelParams, ChannelSettings, DabMode, DabParams, DabTransmissionMode, DecoderEvent,
    DecoderFamily,
};

use super::{
    fic::{FIB_BYTES, FicDecoder},
    fig::{Audio, Ensemble, SubChannel},
    mode::{Mode, TRANSMISSION_MODES},
    msc::{CIF_BITS, SubChannelDecoder, subchannel_range},
    ofdm::{FrameSync, SymbolDemod},
    pacer::Pacer,
    prs::PrsProbe,
    superframe::{AccessUnits, SuperframeAssembler},
    sync::{self, MIN_COHERENCE, SEARCH, Tracker},
};
use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    broadcast_audio::LayerTwoAudio,
    broadcast_media::{BroadcastMedia, Kind as MediaKind},
    check_input_rate,
};

const INPUT_RATE_HZ: f64 = 2_048_000.0;
const BANDWIDTH_HZ: f64 = 1_536_000.0;
const MAX_OFFSET_HZ: f64 = 40_000.0;
const REPORT_FRAMES: u32 = 5;
const REDETECT_FRAMES: u32 = 40;
const LOCK_QUALITY: f32 = 0.5;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dab".to_owned(),
    name: "DAB / DAB+".to_owned(),
    summary: "DAB and DAB+ digital radio".to_owned(),
    family: DecoderFamily::Broadcast,
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

fn params(settings: &ChannelSettings) -> Result<DabParams, ChannelError> {
    match settings.params {
        ChannelParams::Dab(p) => Ok(p),
        ref other => Err(ChannelError::InvalidSettings(format!(
            "dab channel got {} params",
            other.type_id()
        ))),
    }
}

pub fn occupied_band() -> (f64, f64) {
    (-BANDWIDTH_HZ / 2.0, BANDWIDTH_HZ / 2.0)
}

pub fn channel_filter() -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(127, (BANDWIDTH_HZ / 2.0 + MAX_OFFSET_HZ) / INPUT_RATE_HZ),
        1,
    ))
}

struct Selection {
    service: u32,
    subchannel: SubChannel,
    decoder: SubChannelDecoder,
    assembler: Option<SuperframeAssembler>,
    audio: Audio,
    packet: Option<super::packet::Config>,
}

struct Chain {
    transmission: DabTransmissionMode,
    demod: SymbolDemod,
    probe: PrsProbe,
    fic: FicDecoder,
    window: Vec<Complex<f32>>,
}

impl Chain {
    fn new(transmission: DabTransmissionMode) -> Self {
        Self {
            transmission,
            demod: SymbolDemod::for_mode(transmission),
            probe: PrsProbe::for_mode(transmission),
            fic: FicDecoder::for_mode(transmission),
            window: vec![Complex::new(0.0, 0.0); Mode::new(transmission).useful],
        }
    }
}

pub struct DabChannel {
    params: DabParams,
    transmission: Option<DabTransmissionMode>,
    active: DabTransmissionMode,
    spares: Vec<Chain>,
    mode: Mode,
    sync: FrameSync,
    demod: SymbolDemod,
    probe: PrsProbe,
    tracker: Tracker,
    fic: FicDecoder,
    ensemble: Ensemble,
    pending: Vec<Complex<f32>>,
    origin: u64,
    search: Option<(usize, usize)>,
    window: Vec<Complex<f32>>,
    symbols: Vec<Soft>,
    fibs: Vec<[u8; FIB_BYTES]>,
    logical: Vec<u8>,
    selection: Option<Selection>,
    audio: LayerTwoAudio,
    media: BroadcastMedia,
    pacer: Pacer,
    frames: u32,
    frequency_error_hz: f32,
    snr_db: f32,
    superframes: u32,
    units: u32,
    last_format: Option<AccessUnits>,
    locked: bool,
    samples_without_frame: usize,
    unlocked_frames: u32,
}

fn derotate(
    source: &[Complex<f32>],
    target: &mut [Complex<f32>],
    offset: f64,
    cycles_per_sample: f64,
) {
    let mut phase = Complex::<f64>::from_polar(1.0, -TAU * cycles_per_sample * offset);
    let step = Complex::<f64>::from_polar(1.0, -TAU * cycles_per_sample);
    for (out, &sample) in target.iter_mut().zip(source) {
        *out = sample * Complex::new(phase.re as f32, phase.im as f32);
        phase *= step;
    }
}

impl DabChannel {
    fn reset(&mut self) {
        self.sync.reset();
        self.demod.reset();
        self.fic.reset();
        self.ensemble.clear();
        self.discard(self.pending.len());
        self.search = None;
        self.tracker.reset();
        self.forget_mode();
        self.selection = None;
        self.stop_audio();
        self.frames = 0;
        self.frequency_error_hz = 0.0;
        self.snr_db = 0.0;
        self.superframes = 0;
        self.units = 0;
        self.last_format = None;
        self.locked = false;
        self.samples_without_frame = 0;
    }

    fn adopt(&mut self, transmission: DabTransmissionMode) {
        if let Some(spare) = self
            .spares
            .iter_mut()
            .find(|spare| spare.transmission == transmission)
            && transmission != self.active
        {
            std::mem::swap(&mut self.demod, &mut spare.demod);
            std::mem::swap(&mut self.probe, &mut spare.probe);
            std::mem::swap(&mut self.fic, &mut spare.fic);
            std::mem::swap(&mut self.window, &mut spare.window);
            spare.transmission = self.active;
            self.active = transmission;
        }
        self.transmission = Some(self.active);
        self.mode = Mode::new(self.active);
        self.sync = FrameSync::for_mode(self.active);
        self.demod.reset();
        self.fic.reset();
        self.unlocked_frames = 0;
    }

    fn forget_mode(&mut self) {
        if self.params.transmission_mode == DabTransmissionMode::Auto {
            self.transmission = None;
            self.sync = FrameSync::any_mode();
            self.tracker.forget();
            self.unlocked_frames = 0;
        }
    }

    fn discard(&mut self, count: usize) {
        let count = count.min(self.pending.len());
        self.pending.drain(..count);
        self.origin += count as u64;
    }

    fn stop_audio(&mut self) {
        self.audio.reset();
        self.media.reset();
        self.pacer.reset();
    }

    fn collect_audio(&mut self, out: &mut ChannelOutputs) {
        let from = out.audio_pcm.len();
        self.audio.drain(out);
        self.media.drain(out);
        self.pacer.take(out, from);
    }

    fn next_search(&self, null: Option<usize>) -> Option<(usize, usize)> {
        if let Some(next) = self.tracker.next() {
            let at = (next - self.origin as f64).round().max(0.0) as usize;
            let half = self.mode.guard / 2;
            return Some((at.saturating_sub(half), 2 * half));
        }
        let at = self.pending.len().saturating_sub(null?);
        Some((at.saturating_sub(SEARCH), 2 * SEARCH))
    }

    fn detect(&mut self, from: usize, span: usize) -> bool {
        if self.pending.len() < from + span + sync::detect_span() {
            return false;
        }
        match sync::detect(&self.pending, from, span) {
            Some(found) => {
                self.adopt(found);
                self.take_frame()
            }
            None => {
                self.search = None;
                self.discard(from + span);
                false
            }
        }
    }

    fn take_frame(&mut self) -> bool {
        let Some((from, span)) = self.search else {
            return false;
        };
        if self.transmission.is_none() {
            return self.detect(from, span);
        }
        if self.pending.len() < from + span + self.mode.frame_samples() + self.mode.symbol() {
            return false;
        }
        self.search = None;
        if let Some(useful) = self.acquire(from, span)
            && self.demodulate(useful)
        {
            self.discard(useful - self.mode.guard + self.mode.frame_samples());
            return true;
        }
        self.tracker.missed(self.mode);
        self.discard(from + span);
        false
    }

    fn acquire(&mut self, from: usize, span: usize) -> Option<usize> {
        let drift = self.tracker.drift;
        let (coherence, at) = sync::align(self.mode, &self.pending, from, span, drift);
        if coherence < MIN_COHERENCE {
            return None;
        }
        let phase = sync::prefix_phase(self.mode, &self.pending, at, drift);
        let frequency = self.tracker.refined(self.mode, phase);
        let start = at + self.mode.guard - self.mode.backoff();
        let source = self.pending.get(start..start + self.mode.useful)?;
        derotate(source, &mut self.window, 0.0, frequency);
        let shifts = sync::shifts(self.mode, frequency, MAX_OFFSET_HZ / INPUT_RATE_HZ);
        let probe = self.probe.probe(&self.window, shifts)?;
        let useful = usize::try_from(start as i64 + i64::from(probe.first_path)).ok()?;
        if useful < self.mode.guard {
            return None;
        }
        let frequency = frequency + f64::from(probe.shift) / self.mode.useful as f64;
        let origin = self.origin as f64;
        self.tracker.locked(
            self.mode,
            frequency,
            origin + start as f64 - probe.mean_delay,
            origin + (useful - self.mode.guard) as f64,
        );
        self.frequency_error_hz = (frequency * INPUT_RATE_HZ) as f32;
        Some(useful)
    }

    fn demodulate(&mut self, useful: usize) -> bool {
        self.symbols.clear();
        self.demod.reset();
        let first = (useful - self.mode.backoff()) as f64;
        for index in 0..self.mode.symbols {
            let start = sync::symbol_start(self.mode, first, index, self.tracker.drift);
            let whole = start.floor();
            let at = whole as usize;
            let Some(source) = self.pending.get(at..at + self.mode.useful) else {
                return false;
            };
            derotate(
                source,
                &mut self.window,
                whole - useful as f64,
                self.tracker.frequency,
            );
            self.demod
                .demodulate(&self.window, (start - whole) as f32, &mut self.symbols);
        }
        self.snr_db = self.demod.snr_db();
        true
    }

    fn read_fic(&mut self) {
        let end = self.mode.fic_symbols * self.mode.symbol_bits();
        let Some(fic) = self.symbols.get(..end) else {
            return;
        };
        self.fibs.clear();
        let mut fibs = std::mem::take(&mut self.fibs);
        for block in fic.chunks(self.mode.fic_block_bits()) {
            self.fic.block(block, &mut fibs);
        }
        for fib in &fibs {
            self.ensemble.absorb(fib);
        }
        self.fibs = fibs;
    }

    fn choose(&mut self) {
        let wanted = self.params.service_id;
        let chosen = self.ensemble.playable().find(|(service, _)| {
            wanted.is_none_or(|id| id == service.id)
                && if service.data {
                    wanted.is_some()
                } else {
                    match self.params.mode {
                        DabMode::Auto => true,
                        DabMode::Dab => service.audio == Audio::Mp2,
                        DabMode::DabPlus => service.audio == Audio::AacPlus,
                    }
                }
        });
        let Some((service, subchannel)) = chosen else {
            if self.selection.take().is_some() {
                self.stop_audio();
            }
            return;
        };
        let packet = self.ensemble.packet_config(service);
        self.media.mot_app = service.mot_app;
        self.media.service_id = Some(service.id);
        self.media.packet_config = packet;
        if self.selection.as_ref().is_some_and(|current| {
            current.service == service.id
                && current.subchannel == *subchannel
                && current.audio == service.audio
                && current.packet == packet
        }) {
            return;
        }
        let frame_bytes = subchannel.protection.frame_bits() / 8;
        self.selection = Some(Selection {
            service: service.id,
            subchannel: subchannel.clone(),
            decoder: SubChannelDecoder::new(subchannel.protection.clone()),
            assembler: match service.audio {
                Audio::AacPlus => SuperframeAssembler::new(frame_bytes),
                Audio::Mp2 => None,
            },
            audio: service.audio,
            packet,
        });
        self.superframes = 0;
        self.units = 0;
        self.last_format = None;
        self.audio.reset();
        self.media.reset();
        self.pacer.reset();
        self.media.mot_app = service.mot_app;
        self.media.service_id = Some(service.id);
    }

    fn read_msc(&mut self) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        let Some((low, high)) =
            subchannel_range(selection.subchannel.start_cu, selection.subchannel.size_cu)
        else {
            return;
        };
        let base = self.mode.fic_symbols * self.mode.symbol_bits();
        for cif in 0..self.mode.cifs {
            let start = base + cif * CIF_BITS;
            let Some(fragment) = self.symbols.get(start + low..start + high) else {
                return;
            };
            let mut logical = std::mem::take(&mut self.logical);
            let ready = selection.decoder.frame(fragment, &mut logical);
            self.logical = logical;
            if !ready {
                continue;
            }

            if selection.packet.is_some() {
                self.media
                    .push(MediaKind::DabPacket, &self.logical, None, None);
                continue;
            }
            if selection.audio == Audio::Mp2 {
                self.audio.push(&self.logical);
                self.media
                    .push(MediaKind::DabPad, &self.logical, None, None);
            }
            if let Some(assembler) = &mut selection.assembler
                && let Some(units) = assembler.frame(&self.logical)
            {
                self.superframes += 1;
                self.units += units.units.len() as u32;
                if units.dropped > 0 {
                    self.media.audio_gap(units.dropped);
                }
                for unit in &units.units {
                    self.media
                        .push(MediaKind::Latm, unit, None, Some(units.format));
                }
                self.last_format = Some(units);
            }
        }
    }

    fn system(&self) -> BroadcastSystem {
        let generation = self
            .selection
            .as_ref()
            .map_or(self.params.mode, |selection| match selection.audio {
                Audio::AacPlus => DabMode::DabPlus,
                Audio::Mp2 => DabMode::Dab,
            });
        match generation {
            DabMode::DabPlus => BroadcastSystem::DabPlus,
            DabMode::Auto | DabMode::Dab => BroadcastSystem::Dab,
        }
    }

    fn services(&self) -> Vec<BroadcastService> {
        let chosen = self.selection.as_ref().map(|selection| selection.service);
        self.ensemble
            .playable()
            .map(|(service, subchannel)| BroadcastService {
                id: service.id,
                label: service
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("{:04X}", service.id)),
                kind: if service.data {
                    BroadcastServiceKind::Data
                } else {
                    BroadcastServiceKind::Audio
                },
                bitrate_kbps: Some(u32::from(subchannel.bitrate_kbps)),
                language: None,
                selected: chosen == Some(service.id),
            })
            .collect()
    }

    fn decode_frame(&mut self, out: &mut ChannelOutputs) {
        self.samples_without_frame = 0;
        self.read_fic();
        self.choose();
        self.read_msc();
        self.collect_audio(out);
        self.frames += 1;
        if self.frames >= REPORT_FRAMES {
            self.frames = 0;
            self.report(out);
            if self.unlocked_frames >= REDETECT_FRAMES {
                self.forget_mode();
            }
        }
    }

    fn lose_signal(&mut self, out: &mut ChannelOutputs) {
        self.fic.reset();
        self.selection = None;
        self.stop_audio();
        self.snr_db = 0.0;
        self.frequency_error_hz = 0.0;
        self.report(out);
        self.tracker.forget();
        self.forget_mode();
    }

    fn report(&mut self, out: &mut ChannelOutputs) {
        let quality = self.fic.quality();
        self.locked = quality >= LOCK_QUALITY;
        self.unlocked_frames = if self.locked {
            0
        } else {
            self.unlocked_frames.saturating_add(REPORT_FRAMES)
        };
        let selected = self
            .selection
            .as_ref()
            .and_then(|selection| self.ensemble.services.get(&selection.service));
        let text = self.last_format.as_ref().map(|units| {
            let format = units.format;
            let rates = if format.spectral_band_replication {
                format!(
                    "{}→{} kHz",
                    format.core_rate_hz() / 1_000,
                    format.output_rate_hz() / 1_000
                )
            } else {
                format!("{} kHz", format.output_rate_hz() / 1_000)
            };
            format!("{} {rates} {}ch", format.codec(), format.channels())
        });
        out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
            dynamic_label: self.media.dynamic_label.clone(),
            data_groups_ok: self.media.data_groups,
            data_groups_bad: self.media.data_errors,
            data_error: self.media.data_error.clone(),
            system: self.system(),
            locked: self.locked,
            snr_db: self.snr_db,
            frequency_error_hz: self.frequency_error_hz,
            audio_frames_ok: self.audio.frames_ok.saturating_add(self.media.audio_frames),
            audio_frames_bad: self
                .audio
                .frames_bad
                .saturating_add(self.media.audio_errors)
                .saturating_add(self.pacer.dropped_frames),
            audio_error: self
                .audio
                .error
                .map(str::to_owned)
                .or_else(|| self.media.audio_error.clone())
                .or_else(|| {
                    (self.pacer.dropped_frames > 0).then(|| "Audio buffer overflow".to_owned())
                }),
            symbol_rate: self
                .transmission
                .map(|_| INPUT_RATE_HZ / self.mode.useful as f64),
            transmission_mode: self.transmission,
            ensemble_id: self.ensemble.id.map(u32::from),
            ensemble_label: self.ensemble.label.clone(),
            service_id: selected.map(|service| service.id),
            label: selected.and_then(|service| service.label.clone()),
            bitrate_kbps: self
                .selection
                .as_ref()
                .map(|selection| u32::from(selection.subchannel.bitrate_kbps)),
            bit_error_rate: None,
            text,
            frames_ok: self.fic.blocks_ok,
            frames_bad: self.fic.blocks_bad,
            services: self.services(),
            ..BroadcastStatus::default()
        }));
    }
}

impl ChannelRx for DabChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        let automatic = params.transmission_mode == DabTransmissionMode::Auto;
        let transmission = (!automatic).then_some(params.transmission_mode);
        let mode = Mode::new(params.transmission_mode);
        Ok(Self {
            params,
            transmission,
            active: params.transmission_mode,
            spares: if automatic {
                TRANSMISSION_MODES.map(Chain::new).into()
            } else {
                Vec::new()
            },
            mode,
            sync: transmission.map_or_else(FrameSync::any_mode, FrameSync::for_mode),
            demod: SymbolDemod::for_mode(params.transmission_mode),
            probe: PrsProbe::for_mode(params.transmission_mode),
            tracker: Tracker::default(),
            fic: FicDecoder::for_mode(params.transmission_mode),
            ensemble: Ensemble::default(),
            pending: Vec::with_capacity(2 * Mode::new(DabTransmissionMode::I).frame()),
            origin: 0,
            search: None,
            window: vec![Complex::new(0.0, 0.0); mode.useful],
            symbols: Vec::with_capacity(mode.symbols * mode.symbol_bits()),
            fibs: Vec::new(),
            logical: Vec::new(),
            selection: None,
            audio: LayerTwoAudio::new()?,
            media: BroadcastMedia::new()?,
            pacer: Pacer::new(INPUT_RATE_HZ),
            frames: 0,
            frequency_error_hz: 0.0,
            snr_db: 0.0,
            superframes: 0,
            units: 0,
            last_format: None,
            locked: false,
            samples_without_frame: 0,
            unlocked_frames: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = params(&settings)?;
        if wanted.transmission_mode != self.params.transmission_mode {
            *self = Self::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings,
            )?;
            return Ok(());
        }
        let changed =
            wanted.service_id != self.params.service_id || wanted.mode != self.params.mode;
        self.params = wanted;
        if changed {
            self.selection = None;
            self.stop_audio();
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for &sample in iq {
            self.samples_without_frame = self.samples_without_frame.saturating_add(1);
            if self.samples_without_frame >= 3 * self.mode.frame() && self.locked {
                self.lose_signal(out);
            }
            self.pending.push(sample);
            let null = self.sync.push(sample);
            if self.search.is_none() {
                self.search = self.next_search(null);
            }
            if self.search.is_none() && self.pending.len() > 2 * self.mode.frame() {
                self.discard(self.mode.frame());
            }
            if self.take_frame() {
                self.decode_frame(out);
            }
        }
        self.collect_audio(out);
        self.pacer.release(iq.len(), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dab::ofdm::FRAME, synth, testutil::realtime_budget};

    fn settings(service_id: Option<u32>) -> ChannelSettings {
        ChannelSettings {
            frequency_hz: 0.0,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dab(DabParams {
                mode: DabMode::Auto,
                service_id,
                ..DabParams::default()
            }),
            blanker: Default::default(),
        }
    }

    fn channel(service_id: Option<u32>) -> DabChannel {
        DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(service_id),
        )
        .expect("a DAB channel at the descriptor rate")
    }

    fn automatic(service_id: Option<u32>) -> DabChannel {
        let mut settings = settings(service_id);
        if let ChannelParams::Dab(params) = &mut settings.params {
            params.transmission_mode = DabTransmissionMode::Auto;
        }
        DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings,
        )
        .expect("an automatic DAB channel")
    }

    fn run(channel: &mut DabChannel, iq: &[Complex<f32>]) -> ChannelOutputs {
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        out
    }

    fn assert_audio(channel: &mut DabChannel, out: &mut ChannelOutputs) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while channel.media.audio_frames == 0 && std::time::Instant::now() < until {
            channel.media.drain(out);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            channel.media.audio_frames > 0,
            "{:?}",
            channel.media.audio_error
        );
    }

    fn cif_frames(transmission: DabTransmissionMode, cifs: usize) -> usize {
        cifs / Mode::new(transmission).cifs
    }

    fn status(out: &ChannelOutputs) -> &BroadcastStatus {
        out.events
            .iter()
            .rev()
            .find_map(|event| match event {
                DecoderEvent::Broadcast(status) => Some(status),
                _ => None,
            })
            .expect("a broadcast status")
    }

    #[test]
    fn a_generated_ensemble_locks_and_names_its_services() {
        let iq = synth::dab::ensemble(12);
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let status = status(&out);
        assert!(status.locked, "{status:?}");
        assert_eq!(status.ensemble_label.as_deref(), Some("SDR-- test"));
        assert_eq!(status.ensemble_id, Some(0x10CD));
        assert_eq!(status.services.len(), 2);
        assert_eq!(status.services[0].label, "Rust FM");
        assert_eq!(status.services[1].label, "Rust Talk");
        assert!(status.frequency_error_hz.abs() < 5.0);
    }

    #[test]
    fn the_selected_service_yields_dab_plus_access_units() {
        let iq = synth::dab::ensemble(40);
        let mut channel = channel(Some(synth::dab::MUSIC_SERVICE));
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let status = status(&out);
        assert!(status.locked, "{status:?}");
        assert_eq!(status.system, BroadcastSystem::DabPlus);
        assert_eq!(status.service_id, Some(synth::dab::MUSIC_SERVICE));
        assert_eq!(status.label.as_deref(), Some("Rust FM"));
        assert_eq!(status.bitrate_kbps, Some(96));
        assert!(channel.superframes > 0, "no superframe was assembled");
        assert!(channel.units >= 3 * channel.superframes);
        let format = channel.last_format.as_ref().expect("an audio format");
        assert_eq!(format.format.codec(), "HE-AAC");
        assert_eq!(format.format.output_rate_hz(), 48_000);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while channel.media.audio_frames == 0 && std::time::Instant::now() < until {
            channel.media.drain(&mut out);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            channel.media.audio_frames > 0,
            "{:?}",
            channel.media.audio_error
        );
        assert_eq!(
            channel.media.audio_errors, 0,
            "{:?}",
            channel.media.audio_error
        );
        assert_eq!(channel.media.dynamic_label.as_deref(), Some("SDR-- live"));
        assert!(out.events.iter().any(|event| matches!(event, DecoderEvent::BroadcastData(data) if data.name == "slide.png" && data.bytes == include_bytes!("../../../../fixtures/broadcast_audio/slideshow.png"))), "{:?}", channel.media.data_error);
        assert!(out.audio_pcm.iter().any(|sample| sample.abs() > 0.01));
    }

    #[test]
    fn all_transmission_modes_decode_fic_and_msc_independently_of_input_blocks() {
        use sdrmm_wire::DabTransmissionMode::{I, Ii, Iii, Iv};
        for mode in [I, Ii, Iii, Iv] {
            let iq = synth::dab::ensemble_for_mode(mode, 40);
            let mut reference = None;
            for block_size in [997, 16384, iq.len()] {
                let mut channel = channel(Some(synth::dab::MUSIC_SERVICE));
                let mut settings = settings(Some(synth::dab::MUSIC_SERVICE));
                if let ChannelParams::Dab(params) = &mut settings.params {
                    params.transmission_mode = mode;
                }
                channel.apply(settings).expect("mode change");
                let mut out = ChannelOutputs::default();
                for block in iq.chunks(block_size) {
                    channel.process(block, &mut out);
                }
                let status = status(&out);
                assert!(status.locked, "{mode:?}: {status:?}");
                assert_eq!(status.ensemble_id, Some(0x10CD));
                assert_eq!(status.services.len(), 2);
                assert!(channel.superframes > 0, "{mode:?}: no MSC superframe");
                assert_eq!(
                    status.symbol_rate,
                    Some(INPUT_RATE_HZ / channel.mode.useful as f64)
                );
                let counts = (channel.fic.blocks_ok, channel.superframes, channel.units);
                assert_eq!(
                    counts.0,
                    39 * channel.mode.cifs as u32 * channel.mode.fibs_per_block as u32
                );
                if let Some(expected) = reference {
                    assert_eq!(counts, expected, "{mode:?}, block {block_size}");
                }
                reference = Some(counts);
                assert!(channel.pending.len() <= 2 * channel.mode.frame());
            }
        }
    }

    #[test]
    fn changing_transmission_mode_discards_old_ensemble_and_interleaver_state() {
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        channel.process(&synth::dab::ensemble(6), &mut out);
        assert!(status(&out).locked);
        let mut settings = settings(None);
        if let ChannelParams::Dab(params) = &mut settings.params {
            params.transmission_mode = sdrmm_wire::DabTransmissionMode::Iii;
        }
        channel.apply(settings).expect("mode change");
        assert!(channel.ensemble.services.is_empty());
        assert_eq!(channel.fic.blocks_ok, 0);
        assert!(channel.selection.is_none());
        out.reset();
        channel.process(
            &synth::dab::ensemble_for_mode(sdrmm_wire::DabTransmissionMode::Iii, 11),
            &mut out,
        );
        assert!(status(&out).locked);
        assert_eq!(status(&out).symbol_rate, Some(8000.0));
    }

    fn frozen() -> [(DabTransmissionMode, Vec<Complex<f32>>); 3] {
        use sdrmm_wire::DabTransmissionMode::{Ii, Iii, Iv};
        [
            (
                Ii,
                &include_bytes!("../../../../fixtures/dab/mode_ii_reference_2m048.sigmf-data")[..],
            ),
            (
                Iii,
                &include_bytes!("../../../../fixtures/dab/mode_iii_reference_2m048.sigmf-data")[..],
            ),
            (
                Iv,
                &include_bytes!("../../../../fixtures/dab/mode_iv_reference_2m048.sigmf-data")[..],
            ),
        ]
        .map(|(mode, bytes)| {
            let frame: Vec<Complex<f32>> = bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|sample| {
                    Complex::new(
                        f32::from_le_bytes(sample[..4].try_into().expect("I")),
                        f32::from_le_bytes(sample[4..].try_into().expect("Q")),
                    )
                })
                .collect();
            (mode, frame.repeat(7))
        })
    }

    fn assert_reference_ensemble(mode: DabTransmissionMode, status: &BroadcastStatus) {
        assert!(status.locked, "{mode:?}: {status:?}");
        assert_eq!(status.ensemble_id, Some(0x4a2c));
        assert_eq!(status.ensemble_label.as_deref(), Some("Reference DAB"));
        assert_eq!(status.service_id, Some(0xc201));
        assert_eq!(status.label.as_deref(), Some("Reference audio"));
        assert_eq!(status.bitrate_kbps, Some(96));
        assert_eq!(status.frames_bad, 0);
    }

    #[test]
    fn frozen_independent_waveforms_decode_the_expected_ensemble() {
        for (mode, iq) in frozen() {
            let mut channel = channel(None);
            let mut settings = settings(None);
            if let ChannelParams::Dab(params) = &mut settings.params {
                params.transmission_mode = mode;
            }
            channel.apply(settings).expect("mode change");
            let mut out = ChannelOutputs::default();
            for block in iq.chunks(1009) {
                channel.process(block, &mut out);
            }
            assert_reference_ensemble(mode, status(&out));
        }
    }

    #[test]
    fn frozen_waveforms_off_frequency_are_detected_and_decoded() {
        for ((mode, iq), offset_hz) in frozen().into_iter().zip([-27_310.0, 24_480.0, 31_905.0]) {
            let mut channel = automatic(None);
            let out = run(
                &mut channel,
                &crate::testutil::frequency_shift(&iq, offset_hz, INPUT_RATE_HZ),
            );
            let status = status(&out);
            assert_reference_ensemble(mode, status);
            assert_eq!(status.transmission_mode, Some(mode));
            assert!(
                (f64::from(status.frequency_error_hz) - offset_hz).abs() < 2.0,
                "{mode:?}: {} Hz for {offset_hz} Hz",
                status.frequency_error_hz
            );
        }
    }

    #[test]
    fn classic_dab_generation_selects_layer_two_and_produces_audio() {
        let mut channel = channel(None);
        let mut settings = settings(None);
        if let ChannelParams::Dab(params) = &mut settings.params {
            params.mode = DabMode::Dab;
        }
        channel.apply(settings).expect("classic generation");
        let mut out = ChannelOutputs::default();
        for block in synth::dab::ensemble(18).chunks(16384) {
            channel.process(block, &mut out);
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while channel.audio.frames_ok == 0 && std::time::Instant::now() < until {
            channel.audio.drain(&mut out);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            channel.selection.as_ref().expect("selection").service,
            synth::dab::TALK_SERVICE
        );
        assert!(channel.audio.frames_ok > 0, "{:?}", channel.audio.error);
        assert!(out.audio_pcm.iter().any(|value| value.abs() > 0.05));
        assert_eq!(out.audio_rate, 48000);
        assert!(
            out.audio_pcm
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| pair[0] == pair[1])
        );
    }

    #[test]
    fn an_ensemble_at_the_coverage_edge_still_locks_and_reports_its_carrier_to_noise() {
        for (snr_db, floor_db) in [(20.0f32, 17.0f32), (12.0, 9.0), (8.0, 5.0)] {
            let iq = crate::testutil::at_snr(&synth::dab::ensemble(20), snr_db, 7);
            let mut channel = channel(None);
            let mut out = ChannelOutputs::default();
            for block in iq.chunks(16_384) {
                channel.process(block, &mut out);
            }
            let status = status(&out);
            assert!(status.locked, "{snr_db} dB: {status:?}");
            assert_eq!(status.ensemble_label.as_deref(), Some("SDR-- test"));
            assert!(
                (floor_db..=snr_db + 1.0).contains(&status.snr_db),
                "{snr_db} dB in, {} dB reported",
                status.snr_db
            );
        }
    }

    #[test]
    fn an_echo_inside_the_guard_interval_does_not_read_as_noise() {
        const ECHO: usize = 200;
        let clean = synth::dab::ensemble(20);
        let mut echoed = clean.clone();
        for index in ECHO..echoed.len() {
            echoed[index] += clean[index - ECHO] * 0.7;
        }
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        for block in echoed.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let status = status(&out);
        assert!(status.locked, "{status:?}");
        assert_eq!(status.frames_bad, 0, "{status:?}");
        assert!(status.snr_db > 25.0, "echo read as {} dB", status.snr_db);
    }

    #[test]
    fn losing_the_carrier_clears_lock_and_pending_audio() {
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        channel.process(&synth::dab::ensemble(7), &mut out);
        assert!(status(&out).locked);
        out.reset();
        channel.process(&vec![Complex::new(0.0, 0.0); 5 * FRAME], &mut out);
        assert!(!status(&out).locked);
        assert!(channel.selection.is_none());
        channel.process(&synth::dab::ensemble(8), &mut out);
        assert!(status(&out).locked);
    }

    #[test]
    fn packet_mode_mot_crosses_fec_and_the_msc() {
        let mut channel = channel(Some(synth::dab::DATA_SERVICE));
        let iq = synth::dab::ensemble_with_data(sdrmm_wire::DabTransmissionMode::I, 24);
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16384) {
            channel.process(block, &mut out);
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !out
            .events
            .iter()
            .any(|e| matches!(e, DecoderEvent::BroadcastData(_)))
            && std::time::Instant::now() < until
        {
            channel.media.drain(&mut out);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let object = out
            .events
            .iter()
            .find_map(|e| match e {
                DecoderEvent::BroadcastData(object) => Some(object),
                _ => None,
            })
            .expect("MOT object");
        assert_eq!(
            object.bytes,
            include_bytes!("../../../../fixtures/broadcast_audio/slideshow.png")
        );
        assert_eq!(object.media_type, "image/png");
        assert!(out.audio_pcm.is_empty());
    }

    #[test]
    fn noise_never_reports_a_lock() {
        let mut state = 0x51ed_270bu32;
        let iq: Vec<Complex<f32>> = (0..3 * FRAME)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::new(
                    (state >> 16) as f32 / 32_768.0 - 1.0,
                    (state & 0xFFFF) as f32 / 32_768.0 - 1.0,
                )
            })
            .collect();
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        channel.process(&iq, &mut out);
        assert!(out.events.iter().all(|event| !matches!(
            event,
            DecoderEvent::Broadcast(status) if status.locked
        )));
    }

    #[test]
    fn decoding_keeps_ahead_of_the_channel_rate() {
        let iq = synth::dab::ensemble(11);
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        let started = std::time::Instant::now();
        for block in iq.chunks(16_384) {
            out.reset();
            channel.process(block, &mut out);
        }
        let elapsed = started.elapsed().as_secs_f64();
        let seconds = iq.len() as f64 / INPUT_RATE_HZ;
        assert!(
            elapsed < realtime_budget(seconds),
            "{seconds:.2} s of DAB took {elapsed:.2} s"
        );
    }

    #[test]
    fn an_ensemble_six_db_above_the_noise_decodes_its_audio() {
        let clean = synth::dab::ensemble(40);
        let power = clean.iter().map(|s| s.norm_sqr()).sum::<f32>() / clean.len() as f32;
        let variance = power * INPUT_RATE_HZ as f32 / (BANDWIDTH_HZ as f32 * 10f32.powf(0.6));
        let deviation = (variance / 2.0).sqrt();
        let mut state = 0x7777_1234u32;
        let mut uniform = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 + 0.5) / 4_294_967_296.0
        };
        let iq: Vec<_> = clean
            .iter()
            .map(|&sample| {
                let (a, b) = (uniform(), uniform());
                let noise = Complex::from_polar(
                    (-2.0 * a.ln()).sqrt() * deviation,
                    std::f32::consts::TAU * b,
                );
                sample + noise
            })
            .collect();
        let mut channel = channel(Some(synth::dab::MUSIC_SERVICE));
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        assert!(status(&out).locked);
        assert!(
            channel.superframes >= 25,
            "{} superframes",
            channel.superframes
        );
    }

    #[test]
    fn every_mode_is_detected_and_decoded_through_a_large_tuner_offset() {
        use sdrmm_wire::DabTransmissionMode::{I, Ii, Iii, Iv};
        for (transmission, offset_hz) in [
            (I, -31_234.5),
            (Ii, 29_876.0),
            (Iii, -30_150.0),
            (Iv, 28_420.0),
        ] {
            let clean = synth::dab::ensemble_for_mode(transmission, cif_frames(transmission, 40));
            let iq = crate::testutil::frequency_shift(&clean, offset_hz, INPUT_RATE_HZ);
            let mut channel = automatic(Some(synth::dab::MUSIC_SERVICE));
            let mut out = run(&mut channel, &iq);
            let status = status(&out).clone();
            assert!(status.locked, "{transmission:?}: {status:?}");
            assert_eq!(status.transmission_mode, Some(transmission));
            assert_eq!(status.frames_bad, 0, "{transmission:?}");
            assert!(
                (f64::from(status.frequency_error_hz) - offset_hz).abs() < 2.0,
                "{transmission:?}: {} Hz for {offset_hz} Hz",
                status.frequency_error_hz
            );
            assert!(channel.superframes > 0, "{transmission:?}");
            assert_audio(&mut channel, &mut out);
        }
    }

    #[test]
    fn every_mode_survives_clock_drift_echoes_doppler_and_noise() {
        use sdrmm_wire::DabTransmissionMode::{I, Ii, Iii, Iv};
        for (transmission, offset_hz, ppm, doppler_hz) in [
            (I, 12_345.0, 100.0, 20.0),
            (Ii, -25_432.0, -100.0, 120.0),
            (Iii, 21_098.0, 100.0, 200.0),
            (Iv, -18_765.0, -100.0, 60.0),
        ] {
            let mode = Mode::new(transmission);
            let clean = synth::dab::ensemble_for_mode(transmission, cif_frames(transmission, 48));
            let echoes = crate::testutil::multipath(
                &clean,
                &[
                    (0, 1.0, 0.0),
                    (mode.guard / 4, 0.5, doppler_hz),
                    (mode.guard / 2, 0.3, -doppler_hz),
                ],
                INPUT_RATE_HZ,
            );
            let drifted = crate::testutil::sample_clock_offset(&echoes, ppm);
            let shifted = crate::testutil::frequency_shift(&drifted, offset_hz, INPUT_RATE_HZ);
            let iq = crate::testutil::at_snr(&shifted, 15.0, 11);
            let mut channel = automatic(Some(synth::dab::MUSIC_SERVICE));
            let mut out = run(&mut channel, &iq);
            let status = status(&out).clone();
            assert!(status.locked, "{transmission:?}: {status:?}");
            assert_eq!(status.transmission_mode, Some(transmission));
            assert!(
                (f64::from(status.frequency_error_hz) - offset_hz).abs()
                    < 0.05 * INPUT_RATE_HZ / mode.useful as f64,
                "{transmission:?}: {} Hz for {offset_hz} Hz",
                status.frequency_error_hz
            );
            assert!(
                (channel.tracker.drift * 1e6 - ppm).abs() < 10.0,
                "{transmission:?}: {} ppm for {ppm} ppm",
                channel.tracker.drift * 1e6
            );
            assert!(
                status.frames_bad * 20 <= status.frames_ok,
                "{transmission:?}: {status:?}"
            );
            assert!(channel.superframes > 0, "{transmission:?}");
            assert_audio(&mut channel, &mut out);
        }
    }

    #[test]
    fn short_null_symbols_keep_frame_sync_at_low_snr() {
        use sdrmm_wire::DabTransmissionMode::{Ii, Iii};
        for transmission in [Ii, Iii] {
            let mode = Mode::new(transmission);
            let frames = 40;
            let clean = synth::dab::ensemble_for_mode(transmission, frames);
            let shifted = crate::testutil::frequency_shift(&clean, -9_870.0, INPUT_RATE_HZ);
            let iq = crate::testutil::at_snr(&shifted, 5.0, 23);
            let mut channel = automatic(None);
            let out = run(&mut channel, &iq);
            let status = status(&out);
            assert!(status.locked, "{transmission:?}: {status:?}");
            assert_eq!(status.transmission_mode, Some(transmission));
            assert_eq!(status.ensemble_label.as_deref(), Some("SDR-- test"));
            let blocks = channel.fic.blocks_ok + channel.fic.blocks_bad;
            assert!(
                blocks >= ((frames - 3) * mode.cifs * mode.fibs_per_block) as u32,
                "{transmission:?}: {blocks} FIBs"
            );
        }
    }

    #[test]
    fn automatic_mode_follows_a_retune_to_another_mode() {
        use sdrmm_wire::DabTransmissionMode::{Ii, Iii};
        let mut channel = automatic(None);
        let out = run(&mut channel, &synth::dab::ensemble_for_mode(Ii, 8));
        assert_eq!(status(&out).transmission_mode, Some(Ii));
        channel.retuned();
        let out = run(&mut channel, &synth::dab::ensemble_for_mode(Iii, 8));
        let status = status(&out);
        assert!(status.locked, "{status:?}");
        assert_eq!(status.transmission_mode, Some(Iii));
        assert_eq!(status.symbol_rate, Some(8000.0));
    }
}
