use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, Soft, design_lowpass};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, BroadcastStatus, BroadcastSystem, ChannelDescriptor,
    ChannelParams, ChannelSettings, DabMode, DabParams, DecoderEvent,
};

use super::{
    fic::{BLOCK_BITS, FIB_BYTES, FicDecoder},
    fig::{Audio, Ensemble},
    msc::{CIF_BITS, SubChannelDecoder, subchannel_range},
    ofdm::{
        FIC_SYMBOLS, FRAME, FrameSync, GUARD, MSC_SYMBOLS, SYMBOL, SYMBOL_BITS, SYMBOLS,
        SymbolDemod, prefix_offset,
    },
    superframe::{AccessUnits, SuperframeAssembler},
};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 2_048_000.0;
const BANDWIDTH_HZ: f64 = 1_536_000.0;
const SEARCH: usize = 96;
const SEARCH_STRIDE: usize = 4;
const FRAME_SAMPLES: usize = SYMBOLS * SYMBOL;
const REPORT_FRAMES: u32 = 5;
const LOCK_QUALITY: f32 = 0.5;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dab".to_owned(),
    name: "DAB / DAB+".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
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
    start_cu: u16,
    size_cu: u16,
    decoder: SubChannelDecoder,
    assembler: Option<SuperframeAssembler>,
    audio: Audio,
    bitrate_kbps: u16,
}

pub struct DabChannel {
    params: DabParams,
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
    frames: u32,
    frequency_error_hz: f32,
    snr_db: f32,
    superframes: u32,
    units: u32,
    last_format: Option<AccessUnits>,
    locked: bool,
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
        self.frames = 0;
        self.frequency_error_hz = 0.0;
        self.snr_db = 0.0;
        self.superframes = 0;
        self.units = 0;
        self.last_format = None;
        self.locked = false;
    }

    fn align(&self, start: usize) -> usize {
        let mut best = (0.0f32, start);
        let limit = self.pending.len().saturating_sub(SYMBOL);
        for coarse in (0..2 * SEARCH).step_by(SEARCH_STRIDE) {
            let at = start + coarse;
            if at > limit {
                break;
            }
            if let Some((coherence, _)) = prefix_offset(&self.pending[at..])
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
            if let Some((coherence, _)) = prefix_offset(&self.pending[at..])
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

    fn take_frame(&mut self) -> Option<Vec<Complex<f32>>> {
        let start = self.frame_start?;
        if self.pending.len() < start + 2 * SEARCH + FRAME_SAMPLES {
            return None;
        }
        let aligned = self.align(start);
        let (_, offset) = prefix_offset(&self.pending[aligned..])?;
        self.frequency_error_hz = offset * INPUT_RATE_HZ as f32;
        let mut frame = self.pending[aligned..aligned + FRAME_SAMPLES].to_vec();
        Self::derotate(&mut frame, offset);
        self.pending.drain(..aligned + FRAME_SAMPLES);
        self.frame_start = None;
        Some(frame)
    }

    fn demodulate(&mut self, frame: &[Complex<f32>]) {
        self.symbols.clear();
        self.demod.reset();
        for index in 0..SYMBOLS {
            let start = index * SYMBOL;
            let mut bits = std::mem::take(&mut self.symbols);
            self.demod
                .demodulate(&frame[start..start + SYMBOL], &mut bits);
            self.symbols = bits;
        }
        self.snr_db = self.demod.snr_db();
    }

    fn read_fic(&mut self) {
        let start = (FIC_SYMBOLS.start - 1) * SYMBOL_BITS;
        let end = (FIC_SYMBOLS.end - 1) * SYMBOL_BITS;
        self.fibs.clear();
        let mut fibs = std::mem::take(&mut self.fibs);
        for block in self.symbols[start..end].chunks(BLOCK_BITS) {
            self.fic.block(block, &mut fibs);
        }
        for fib in &fibs {
            self.ensemble.absorb(fib);
        }
        self.fibs = fibs;
    }

    fn choose(&mut self) {
        let wanted = self.params.service_id;
        let Some((service, subchannel)) = self.ensemble.pick(wanted) else {
            return;
        };
        if self
            .selection
            .as_ref()
            .is_some_and(|current| current.service == service.id)
        {
            return;
        }
        let frame_bytes = subchannel.protection.frame_bits() / 8;
        self.selection = Some(Selection {
            service: service.id,
            start_cu: subchannel.start_cu,
            size_cu: subchannel.size_cu,
            decoder: SubChannelDecoder::new(subchannel.protection.clone()),
            assembler: match service.audio {
                Audio::AacPlus => SuperframeAssembler::new(frame_bytes),
                Audio::Mp2 => None,
            },
            audio: service.audio,
            bitrate_kbps: subchannel.bitrate_kbps,
        });
        self.superframes = 0;
        self.units = 0;
        self.last_format = None;
    }

    fn read_msc(&mut self) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        let Some((low, high)) = subchannel_range(selection.start_cu, selection.size_cu) else {
            return;
        };
        let base = (MSC_SYMBOLS.start - 1) * SYMBOL_BITS;
        for cif in 0..4 {
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

            if let Some(assembler) = &mut selection.assembler
                && let Some(units) = assembler.frame(&self.logical)
            {
                self.superframes += 1;
                self.units += units.units.len() as u32;
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
            format!(
                "{} {} kHz {}ch",
                units.format.codec(),
                units.format.output_rate_hz() / 1_000,
                units.format.channels()
            )
        });
        out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
            system: self.system(),
            locked: self.locked,
            snr_db: self.snr_db,
            frequency_error_hz: self.frequency_error_hz,
            symbol_rate: Some(1_000.0),
            ensemble_id: self.ensemble.id.map(u32::from),
            ensemble_label: self.ensemble.label.clone(),
            service_id: selected.map(|service| service.id),
            label: selected.and_then(|service| service.label.clone()),
            bitrate_kbps: self
                .selection
                .as_ref()
                .map(|selection| u32::from(selection.bitrate_kbps)),
            bit_error_rate: Some(1.0 - quality),
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
        Ok(Self {
            params: params(&settings)?,
            sync: FrameSync::new(),
            demod: SymbolDemod::new(),
            fic: FicDecoder::new(),
            ensemble: Ensemble::default(),
            pending: Vec::with_capacity(2 * FRAME),
            frame_start: None,
            symbols: Vec::with_capacity(SYMBOLS * SYMBOL_BITS),
            fibs: Vec::new(),
            logical: Vec::new(),
            selection: None,
            frames: 0,
            frequency_error_hz: 0.0,
            snr_db: 0.0,
            superframes: 0,
            units: 0,
            last_format: None,
            locked: false,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = params(&settings)?;
        let changed = wanted.service_id != self.params.service_id;
        self.params = wanted;
        if changed {
            self.selection = None;
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for &sample in iq {
            self.pending.push(sample);
            if self.sync.push(sample) && self.frame_start.is_none() {
                let at = self.pending.len().saturating_sub(1);
                self.frame_start = Some(at.saturating_sub(GUARD));
            }
            if self.frame_start.is_none() && self.pending.len() > 2 * FRAME {
                self.pending.drain(..FRAME);
            }
        }
        while let Some(frame) = self.take_frame() {
            self.demodulate(&frame);
            self.read_fic();
            self.choose();
            self.read_msc();
            self.frames += 1;
            if self.frames >= REPORT_FRAMES {
                self.frames = 0;
                self.report(out);
            }
        }
        if self.pending.len() > 3 * FRAME {
            let excess = self.pending.len() - 2 * FRAME;
            self.pending.drain(..excess);
            self.frame_start = self.frame_start.and_then(|start| start.checked_sub(excess));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgen;

    fn settings(service_id: Option<u32>) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Dab(DabParams {
                mode: DabMode::Auto,
                service_id,
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
        assert_eq!(status.ensemble_label.as_deref(), Some("sdr-- test"));
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
        assert_eq!(format.format.output_rate_hz(), 96_000);
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
            elapsed < seconds,
            "{seconds:.2} s of DAB took {elapsed:.2} s"
        );
    }
}
