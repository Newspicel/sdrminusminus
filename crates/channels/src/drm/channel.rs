use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, BroadcastStatus, BroadcastSystem, ChannelDescriptor,
    ChannelSettings, DecoderEvent, DecoderFamily, DrmMode, DrmParams,
};

use super::{
    DRM_PLUS_BANDWIDTH_HZ, INPUT_RATE_HZ,
    coding::Qam,
    fac::language,
    mode::Robustness,
    ofdm::{ACQUIRE_SAMPLES, Acquired, Demodulator, Probe},
    params,
    receiver::Receiver,
};
use crate::{
    ChannelCtx, ChannelError, ChannelOutputs, ChannelRx, broadcast_media::BroadcastMedia,
    check_input_rate, dab::pacer::Pacer,
};

const DECIMATION: usize = 4;
const DECIMATOR_TAPS: usize = 127;
const DECIMATOR_CUTOFF_HZ: f64 = 17_000.0;
const ACQUIRE_INPUT: usize = ACQUIRE_SAMPLES * DECIMATION;
const LOST_INPUT: usize = 4 * ACQUIRE_INPUT;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "drm".to_owned(),
    name: "DRM30 / DRM+".to_owned(),
    summary: "Digital Radio Mondiale receiver".to_owned(),
    family: DecoderFamily::Broadcast,
    bandwidth_hz: DRM_PLUS_BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

struct Locked {
    demod: Demodulator,
    receiver: Receiver,
}

pub struct DrmChannel {
    params: DrmParams,
    decimator: Decimator,
    decimated: Vec<Complex<f32>>,
    probes: Vec<Probe>,
    plus_probe: Probe,
    time: u64,
    plus_time: u64,
    acquiring: usize,
    locked: Option<Locked>,
    media: BroadcastMedia,
    pacer: Pacer,
    silent: usize,
    frames: usize,
}

fn decimator() -> Decimator {
    Decimator::new(
        &design_lowpass(DECIMATOR_TAPS, DECIMATOR_CUTOFF_HZ / INPUT_RATE_HZ),
        DECIMATION,
    )
}

impl DrmChannel {
    fn reset(&mut self) {
        self.decimator.reset();
        for probe in &mut self.probes {
            probe.reset();
        }
        self.plus_probe.reset();
        self.acquiring = 0;
        self.locked = None;
        self.media.reset();
        self.pacer.reset();
        self.silent = 0;
        self.frames = 0;
    }

    fn drm30(&self) -> bool {
        self.params.mode != DrmMode::DrmPlus
    }

    fn plus(&self) -> bool {
        self.params.mode != DrmMode::Drm30
    }

    fn feed(&mut self, iq: &[Complex<f32>]) {
        let plus_mode = self.locked.as_ref().map(|locked| locked.demod.mode.plus());
        if self.plus() {
            for &sample in iq {
                match (&mut self.locked, plus_mode) {
                    (Some(locked), Some(true)) => locked.demod.push(sample, self.plus_time),
                    (None, _) => self.plus_probe.push(sample, self.plus_time),
                    _ => {}
                }
                self.plus_time += 1;
            }
        }
        if self.drm30() {
            let mut decimated = std::mem::take(&mut self.decimated);
            self.decimator.process(iq, &mut decimated);
            for &sample in &decimated {
                match (&mut self.locked, plus_mode) {
                    (Some(locked), Some(false)) => locked.demod.push(sample, self.time),
                    (None, _) => {
                        for probe in &mut self.probes {
                            probe.push(sample, self.time);
                        }
                    }
                    _ => {}
                }
                self.time += 1;
            }
            self.decimated = decimated;
        }
    }

    fn acquire(&mut self, out: &mut ChannelOutputs) {
        if self.acquiring < ACQUIRE_INPUT {
            return;
        }
        self.acquiring = 0;
        let mut best: Option<Acquired> = None;
        if self.drm30() {
            for probe in &self.probes {
                if let Some(found) = probe.result(self.time) {
                    best = Some(match best {
                        Some(current) if current.coherence >= found.coherence => current,
                        _ => found,
                    });
                }
            }
        }
        if self.plus()
            && let Some(found) = self.plus_probe.result(self.plus_time)
            && best.is_none_or(|current| found.coherence > current.coherence)
        {
            best = Some(found);
        }
        for probe in &mut self.probes {
            probe.reset();
        }
        self.plus_probe.reset();
        match best {
            Some(found) => {
                let mut receiver = Receiver::new(found.mode);
                receiver.wanted = self.params.service;
                self.locked = Some(Locked {
                    demod: Demodulator::new(found, found.mode.widest()),
                    receiver,
                });
                self.silent = 0;
            }
            None => self.report(out),
        }
    }

    fn demodulate(&mut self, out: &mut ChannelOutputs) {
        let mut report = false;
        if let Some(locked) = self.locked.as_mut() {
            while let Some(ready) = locked.demod.next() {
                if !ready {
                    continue;
                }
                let estimator = locked.demod.estimator();
                let symbol = estimator.symbol;
                locked
                    .receiver
                    .store(symbol, &estimator.equalized, &estimator.weights);
                if symbol + 1 < locked.demod.mode.symbols() {
                    continue;
                }
                self.silent = 0;
                if let Some(occupancy) = locked.receiver.frame(&mut self.media) {
                    locked.demod.set_occupancy(occupancy);
                }
                if locked.receiver.changed {
                    locked.receiver.changed = false;
                    self.media.reset();
                    self.pacer.reset();
                    self.media.service_id = locked
                        .receiver
                        .selection
                        .as_ref()
                        .and_then(|selection| {
                            locked.receiver.services[usize::from(selection.short_id)]
                        })
                        .map(|service| service.id);
                }
                self.frames = self.frames.wrapping_add(1);
                report |= self.frames.is_multiple_of(locked.demod.mode.frames());
            }
        }
        if report {
            self.report(out);
        }
        let lost = self
            .locked
            .as_ref()
            .is_some_and(|locked| locked.demod.lost || self.silent > LOST_INPUT);
        if lost {
            self.locked = None;
            self.media.reset();
            self.pacer.reset();
            self.report(out);
        }
    }

    fn collect_audio(&mut self, out: &mut ChannelOutputs) {
        let from = out.audio_pcm.len();
        self.media.drain(out);
        self.pacer.take(out, from);
    }

    fn services(&self, locked: &Locked) -> Vec<BroadcastService> {
        let receiver = &locked.receiver;
        let frame_seconds = if locked.demod.mode.plus() { 0.1 } else { 0.4 };
        let chosen = receiver
            .selection
            .as_ref()
            .map(|selection| selection.short_id);
        (0..4u8)
            .filter_map(|id| {
                let service = receiver.services[usize::from(id)]?;
                let index = usize::from(id);
                let bitrate = receiver.sdc.audio[index]
                    .and_then(|audio| {
                        receiver
                            .sdc
                            .multiplex
                            .as_ref()?
                            .streams
                            .get(usize::from(audio.stream))
                            .copied()
                    })
                    .map(|stream| {
                        (f64::from(stream.higher + stream.lower) * 8.0 / frame_seconds / 1000.0)
                            .round() as u32
                    });
                Some(BroadcastService {
                    id: u32::from(id),
                    label: receiver.sdc.labels[index]
                        .clone()
                        .unwrap_or_else(|| format!("{:06X}", service.id)),
                    kind: if service.data {
                        BroadcastServiceKind::Data
                    } else {
                        BroadcastServiceKind::Audio
                    },
                    bitrate_kbps: bitrate,
                    language: receiver.sdc.languages[index]
                        .as_ref()
                        .map(|(code, _)| code.clone())
                        .filter(|code| !code.is_empty())
                        .or_else(|| {
                            Some(language(service.language).to_owned())
                                .filter(|name| !name.is_empty())
                        }),
                    selected: chosen == Some(id),
                })
            })
            .collect()
    }

    fn report(&mut self, out: &mut ChannelOutputs) {
        let status = match &self.locked {
            Some(locked) => self.locked_status(locked),
            None => BroadcastStatus {
                system: if self.params.mode == DrmMode::DrmPlus {
                    BroadcastSystem::DrmPlus
                } else {
                    BroadcastSystem::Drm30
                },
                ..BroadcastStatus::default()
            },
        };
        out.events.push(DecoderEvent::Broadcast(status));
    }

    fn locked_status(&self, locked: &Locked) -> BroadcastStatus {
        let receiver = &locked.receiver;
        let mode = locked.demod.mode;
        let selection = receiver.selection.as_ref();
        let service =
            selection.and_then(|selection| receiver.services[usize::from(selection.short_id)]);
        let fac = receiver.fac;
        let code_rate = fac.and_then(|fac| {
            let multiplex = receiver.sdc.multiplex.as_ref()?;
            let rate = overall_rate(fac.msc, fac.plus, multiplex.protection_lower)?;
            Some(format!("{} {rate}", fac.msc.label()))
        });
        let text = fac.map(|fac| {
            let bandwidth = occupancy_khz(mode, fac.occupancy);
            match selection {
                Some(selection) => format!(
                    "Mode {} · {bandwidth} kHz · {}",
                    mode.label(),
                    selection.config.describe()
                ),
                None => format!("Mode {} · {bandwidth} kHz", mode.label()),
            }
        });
        let services = self.services(locked);
        let bitrate = selection.and_then(|selection| {
            services
                .iter()
                .find(|entry| entry.id == u32::from(selection.short_id))
                .and_then(|entry| entry.bitrate_kbps)
        });
        BroadcastStatus {
            dynamic_label: receiver.text.message.clone(),
            system: if mode.plus() {
                BroadcastSystem::DrmPlus
            } else {
                BroadcastSystem::Drm30
            },
            locked: locked.demod.locked() && receiver.fac_locked(),
            snr_db: receiver.snr_db,
            frequency_error_hz: locked.demod.frequency_offset_hz() as f32,
            symbol_rate: Some(mode.rate_hz() / mode.symbol() as f64),
            service_id: service.map(|service| service.id),
            label: selection
                .and_then(|selection| receiver.sdc.labels[usize::from(selection.short_id)].clone()),
            code_rate,
            bitrate_kbps: bitrate,
            text,
            frames_ok: receiver.fac_ok,
            frames_bad: receiver.fac_bad,
            data_groups_ok: receiver.sdc_ok,
            data_groups_bad: receiver.sdc_bad,
            audio_frames_ok: self.media.audio_frames,
            audio_frames_bad: self
                .media
                .audio_errors
                .saturating_add(self.pacer.dropped_frames),
            audio_error: self.media.audio_error.clone().or_else(|| {
                (self.pacer.dropped_frames > 0).then(|| "Audio buffer overflow".to_owned())
            }),
            services,
            ..BroadcastStatus::default()
        }
    }
}

fn overall_rate(qam: Qam, plus: bool, protection: u8) -> Option<&'static str> {
    Some(match (qam, plus, protection) {
        (Qam::Q64, false, 0) | (Qam::Q16, false, 0) | (Qam::Q16, true, 2) => "0.5",
        (Qam::Q64, false, 1) => "0.6",
        (Qam::Q64, false, 2) => "0.71",
        (Qam::Q64, false, 3) => "0.78",
        (Qam::Q16, false, 1) | (Qam::Q16, true, 3) => "0.62",
        (Qam::Q16, true, 0) => "0.33",
        (Qam::Q16, true, 1) => "0.41",
        (Qam::Q4, true, 0) => "0.25",
        (Qam::Q4, true, 1) => "0.33",
        (Qam::Q4, true, 2) => "0.4",
        (Qam::Q4, true, 3) => "0.5",
        _ => return None,
    })
}

fn occupancy_khz(mode: Robustness, occupancy: u8) -> f32 {
    if mode.plus() {
        return 100.0;
    }
    [4.5, 5.0, 9.0, 10.0, 18.0, 20.0]
        .get(usize::from(occupancy))
        .copied()
        .unwrap_or(0.0)
}

impl ChannelRx for DrmChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        Ok(Self {
            params: params(&settings)?,
            decimator: decimator(),
            decimated: Vec::with_capacity(16_384),
            probes: [Robustness::A, Robustness::B, Robustness::C, Robustness::D]
                .into_iter()
                .map(Probe::new)
                .collect(),
            plus_probe: Probe::new(Robustness::E),
            time: 0,
            plus_time: 0,
            acquiring: 0,
            locked: None,
            media: BroadcastMedia::new()?,
            pacer: Pacer::new(INPUT_RATE_HZ),
            silent: 0,
            frames: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = params(&settings)?;
        let restart =
            wanted.mode != self.params.mode || wanted.bandwidth_hz != self.params.bandwidth_hz;
        self.params = wanted;
        if restart {
            self.reset();
        } else if let Some(locked) = self.locked.as_mut() {
            locked.receiver.wanted = wanted.service;
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.feed(iq);
        if self.locked.is_none() {
            self.acquiring += iq.len();
            self.acquire(out);
        } else {
            self.silent += iq.len();
        }
        self.demodulate(out);
        self.collect_audio(out);
        self.pacer.release(iq.len(), out);
    }
}

#[cfg(test)]
mod tests;
