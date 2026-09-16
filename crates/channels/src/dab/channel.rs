use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Soft, design_lowpass};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, BroadcastStatus, BroadcastSystem, ChannelDescriptor,
    ChannelParams, ChannelSettings, DabMode, DabParams, DecoderEvent,
};

use super::{
    fic::{FIB_BYTES, FicDecoder},
    fig::{Audio, Ensemble, SubChannel},
    mode::Mode,
    msc::{CIF_BITS, SubChannelDecoder, subchannel_range},
    ofdm::{FrameSync, SymbolDemod, prefix_offset_for_mode},
    superframe::{AccessUnits, SuperframeAssembler},
};
use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    broadcast_audio::LayerTwoAudio,
    broadcast_media::{BroadcastMedia, Kind as MediaKind},
    check_input_rate,
};

const INPUT_RATE_HZ: f64 = 2_048_000.0;
const BANDWIDTH_HZ: f64 = 1_536_000.0;
const SEARCH: usize = 96;
const SEARCH_STRIDE: usize = 4;
const REPORT_FRAMES: u32 = 5;
const LOCK_QUALITY: f32 = 0.5;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dab".to_owned(),
    name: "DAB / DAB+".to_owned(),
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
        &design_lowpass(127, BANDWIDTH_HZ / 2.0 / INPUT_RATE_HZ),
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

pub struct DabChannel {
    params: DabParams,
    mode: Mode,
    frame: Vec<Complex<f32>>,
    sync: FrameSync,
    demod: SymbolDemod,
    fic: FicDecoder,
    ensemble: Ensemble,
    pending: Vec<Complex<f32>>,
    frame_start: Option<usize>,
    symbols: Vec<Soft>,
    fibs: Vec<[u8; FIB_BYTES]>,
    logical: Vec<u8>,
    selection: Option<Selection>,
    audio: LayerTwoAudio,
    media: BroadcastMedia,
    frames: u32,
    frequency_error_hz: f32,
    snr_db: f32,
    superframes: u32,
    units: u32,
    last_format: Option<AccessUnits>,
    locked: bool,
    samples_without_frame: usize,
}

impl DabChannel {
    fn reset(&mut self) {
        self.sync.reset();
        self.demod.reset();
        self.fic.reset();
        self.ensemble.clear();
        self.pending.clear();
        self.frame_start = None;
        self.selection = None;
        self.audio.reset();
        self.media.reset();
        self.frames = 0;
        self.frequency_error_hz = 0.0;
        self.snr_db = 0.0;
        self.superframes = 0;
        self.units = 0;
        self.last_format = None;
        self.locked = false;
        self.samples_without_frame = 0;
    }

    fn align(&self, start: usize) -> usize {
        let mut best = (0.0f32, start);
        let limit = self.pending.len().saturating_sub(self.mode.symbol());
        for coarse in (0..2 * SEARCH).step_by(SEARCH_STRIDE) {
            let at = start + coarse;
            if at > limit {
                break;
            }
            if let Some((coherence, _)) = prefix_offset_for_mode(self.mode, &self.pending[at..])
                && coherence > best.0
            {
                best = (coherence, at);
            }
        }
        let low = best.1.saturating_sub(SEARCH_STRIDE);
        for at in low..=best.1 + SEARCH_STRIDE {
            if at > limit {
                break;
            }
            if let Some((coherence, _)) = prefix_offset_for_mode(self.mode, &self.pending[at..])
                && coherence > best.0
            {
                best = (coherence, at);
            }
        }
        best.1
    }

    fn derotate(frame: &mut [Complex<f32>], cycles_per_sample: f32) {
        let mut phase = Complex::new(1.0f32, 0.0);
        let step = Complex::from_polar(1.0, -2.0 * std::f32::consts::PI * cycles_per_sample);
        for sample in frame {
            *sample *= phase;
            phase *= step;
            phase /= phase.norm().max(f32::EPSILON);
        }
    }

    fn take_frame(&mut self) -> bool {
        let Some(start) = self.frame_start else {
            return false;
        };
        if self.pending.len() < start + 2 * SEARCH + self.mode.frame_samples() {
            return false;
        }
        let aligned = self.align(start);
        let Some((_, offset)) = prefix_offset_for_mode(self.mode, &self.pending[aligned..]) else {
            return false;
        };
        self.frequency_error_hz = offset * INPUT_RATE_HZ as f32;
        self.frame.clear();
        self.frame
            .extend_from_slice(&self.pending[aligned..aligned + self.mode.frame_samples()]);
        Self::derotate(&mut self.frame, offset);
        self.pending.drain(..aligned + self.mode.frame_samples());
        self.frame_start = None;
        true
    }

    fn demodulate(&mut self) {
        self.symbols.clear();
        self.demod.reset();
        for symbol in self.frame.chunks_exact(self.mode.symbol()) {
            self.demod.demodulate(symbol, &mut self.symbols);
        }
        self.snr_db = self.demod.snr_db();
    }

    fn read_fic(&mut self) {
        let end = self.mode.fic_symbols * self.mode.symbol_bits();
        self.fibs.clear();
        let mut fibs = std::mem::take(&mut self.fibs);
        for block in self.symbols[..end].chunks(self.mode.fic_block_bits()) {
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
                self.audio.reset();
                self.media.reset();
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

    fn report(&mut self, out: &mut ChannelOutputs) {
        let quality = self.fic.quality();
        self.locked = quality >= LOCK_QUALITY;
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
                .saturating_add(self.media.audio_errors),
            audio_error: self
                .audio
                .error
                .map(str::to_owned)
                .or_else(|| self.media.audio_error.clone()),
            symbol_rate: Some(INPUT_RATE_HZ / self.mode.useful as f64),
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
        let mode = Mode::new(params.transmission_mode);
        Ok(Self {
            params,
            mode,
            frame: Vec::with_capacity(mode.frame_samples()),
            sync: FrameSync::for_mode(params.transmission_mode),
            demod: SymbolDemod::for_mode(params.transmission_mode),
            fic: FicDecoder::for_mode(params.transmission_mode),
            ensemble: Ensemble::default(),
            pending: Vec::with_capacity(2 * mode.frame()),
            frame_start: None,
            symbols: Vec::with_capacity(mode.symbols * mode.symbol_bits()),
            fibs: Vec::new(),
            logical: Vec::new(),
            selection: None,
            audio: LayerTwoAudio::new()?,
            media: BroadcastMedia::new()?,
            frames: 0,
            frequency_error_hz: 0.0,
            snr_db: 0.0,
            superframes: 0,
            units: 0,
            last_format: None,
            locked: false,
            samples_without_frame: 0,
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
            self.audio.reset();
            self.media.reset();
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
                self.fic.reset();
                self.selection = None;
                self.audio.reset();
                self.media.reset();
                self.snr_db = 0.0;
                self.frequency_error_hz = 0.0;
                self.report(out);
            }
            self.pending.push(sample);
            if self.sync.push(sample) && self.frame_start.is_none() {
                let at = self.pending.len().saturating_sub(1);
                self.frame_start = Some(at.saturating_sub(SEARCH));
            }
            if self.frame_start.is_none() && self.pending.len() > 2 * self.mode.frame() {
                self.pending.drain(..self.mode.frame());
            }
            if self.take_frame() {
                self.samples_without_frame = 0;
                self.demodulate();
                self.read_fic();
                self.choose();
                self.read_msc();
                self.audio.drain(out);
                self.media.drain(out);
                self.frames += 1;
                if self.frames >= REPORT_FRAMES {
                    self.frames = 0;
                    self.report(out);
                }
            }
        }
        self.audio.drain(out);
        self.media.drain(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dab::ofdm::FRAME, testgen, testutil::realtime_budget};

    fn settings(service_id: Option<u32>) -> ChannelSettings {
        ChannelSettings {
            frequency_hz: 0.0,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dab(DabParams {
                mode: DabMode::Auto,
                service_id,
                ..DabParams::default()
            }),
            audio: Default::default(),
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
        let iq = testgen::dab::ensemble(12);
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
        let iq = testgen::dab::ensemble(40);
        let mut channel = channel(Some(testgen::dab::MUSIC_SERVICE));
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let status = status(&out);
        assert!(status.locked, "{status:?}");
        assert_eq!(status.system, BroadcastSystem::DabPlus);
        assert_eq!(status.service_id, Some(testgen::dab::MUSIC_SERVICE));
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
            let iq = testgen::dab::ensemble_for_mode(mode, 40);
            let mut reference = None;
            for block_size in [997, 16384, iq.len()] {
                let mut channel = channel(Some(testgen::dab::MUSIC_SERVICE));
                let mut settings = settings(Some(testgen::dab::MUSIC_SERVICE));
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
        channel.process(&testgen::dab::ensemble(6), &mut out);
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
            &testgen::dab::ensemble_for_mode(sdrmm_wire::DabTransmissionMode::Iii, 11),
            &mut out,
        );
        assert!(status(&out).locked);
        assert_eq!(status(&out).symbol_rate, Some(8000.0));
    }

    #[test]
    fn frozen_independent_waveforms_decode_the_expected_ensemble() {
        use sdrmm_wire::DabTransmissionMode::{Ii, Iii, Iv};
        for (mode, bytes) in [
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
        ] {
            let iq: Vec<Complex<f32>> = bytes
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
            let mut channel = channel(None);
            let mut settings = settings(None);
            if let ChannelParams::Dab(params) = &mut settings.params {
                params.transmission_mode = mode;
            }
            channel.apply(settings).expect("mode change");
            let mut out = ChannelOutputs::default();
            for _ in 0..7 {
                for block in iq.chunks(1009) {
                    channel.process(block, &mut out);
                }
            }
            let status = status(&out);
            assert!(status.locked, "{mode:?}: {status:?}");
            assert_eq!(status.ensemble_id, Some(0x4a2c));
            assert_eq!(status.ensemble_label.as_deref(), Some("Reference DAB"));
            assert_eq!(status.service_id, Some(0xc201));
            assert_eq!(status.label.as_deref(), Some("Reference audio"));
            assert_eq!(status.bitrate_kbps, Some(96));
            assert_eq!(status.frames_bad, 0);
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
        for block in testgen::dab::ensemble(18).chunks(16384) {
            channel.process(block, &mut out);
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while channel.audio.frames_ok == 0 && std::time::Instant::now() < until {
            channel.audio.drain(&mut out);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            channel.selection.as_ref().expect("selection").service,
            testgen::dab::TALK_SERVICE
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
    fn losing_the_carrier_clears_lock_and_pending_audio() {
        let mut channel = channel(None);
        let mut out = ChannelOutputs::default();
        channel.process(&testgen::dab::ensemble(7), &mut out);
        assert!(status(&out).locked);
        out.reset();
        channel.process(&vec![Complex::new(0.0, 0.0); 5 * FRAME], &mut out);
        assert!(!status(&out).locked);
        assert!(channel.selection.is_none());
        channel.process(&testgen::dab::ensemble(8), &mut out);
        assert!(status(&out).locked);
    }

    #[test]
    fn packet_mode_mot_crosses_fec_and_the_msc() {
        let mut channel = channel(Some(testgen::dab::DATA_SERVICE));
        let iq = testgen::dab::ensemble_with_data(sdrmm_wire::DabTransmissionMode::I, 24);
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
        let iq = testgen::dab::ensemble(11);
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
}
