use std::{f32::consts::PI, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass, flat_bandwidth_hz};
use sdrmm_wire::{
    BroadcastStatus, BroadcastSystem, ChannelDescriptor, ChannelParams, ChannelSettings,
    DecoderEvent, DrmMode, DrmParams,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 192_000.0;
const DRM30_RATE_HZ: f64 = 48_000.0;
const MIN_DRM30_BANDWIDTH_HZ: f64 = 4_500.0;
const MAX_DRM30_BANDWIDTH_HZ: f64 = 20_000.0;
const DRM_PLUS_BANDWIDTH_HZ: f64 = 100_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "drm".to_owned(),
    name: "DRM30 / DRM+ acquisition".to_owned(),
    bandwidth_hz: DRM_PLUS_BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy, Default)]
struct Pair {
    corr: Complex<f32>,
    carrier: Complex<f32>,
    step: Complex<f32>,
    current_power: f32,
    delayed_power: f32,
}

struct CpProbe {
    useful: usize,
    history: Vec<Complex<f32>>,
    history_at: usize,
    history_filled: usize,
    pairs: Vec<Pair>,
    pair_at: usize,
    pair_filled: usize,
    sum: Pair,
    best: f32,
    best_phase: f32,
    best_carrier: f32,
    best_tone: f32,
    previous: Option<Complex<f32>>,
}

impl CpProbe {
    fn new(useful: usize, guard: usize) -> Self {
        Self {
            useful,
            history: vec![Complex::new(0.0, 0.0); useful],
            history_at: 0,
            history_filled: 0,
            pairs: vec![Pair::default(); guard],
            pair_at: 0,
            pair_filled: 0,
            sum: Pair::default(),
            best: 0.0,
            best_phase: 0.0,
            best_carrier: 0.0,
            best_tone: 0.0,
            previous: None,
        }
    }

    fn push(&mut self, sample: Complex<f32>) {
        let previous = self.previous.replace(sample);
        let delayed = self.history[self.history_at];
        self.history[self.history_at] = sample;
        self.history_at = (self.history_at + 1) % self.history.len();
        if self.history_filled < self.history.len() {
            self.history_filled += 1;
            return;
        }

        let pair = Pair {
            corr: sample * delayed.conj(),
            carrier: sample,
            step: previous
                .filter(|value| value.norm_sqr() > f32::EPSILON)
                .map_or(Complex::new(0.0, 0.0), |value| {
                    sample * value.conj() / (sample.norm() * value.norm()).max(f32::EPSILON)
                }),
            current_power: sample.norm_sqr(),
            delayed_power: delayed.norm_sqr(),
        };
        if self.pair_filled == self.pairs.len() {
            let old = self.pairs[self.pair_at];
            self.sum.corr -= old.corr;
            self.sum.carrier -= old.carrier;
            self.sum.step -= old.step;
            self.sum.current_power -= old.current_power;
            self.sum.delayed_power -= old.delayed_power;
        } else {
            self.pair_filled += 1;
        }
        self.pairs[self.pair_at] = pair;
        self.pair_at = (self.pair_at + 1) % self.pairs.len();
        self.sum.corr += pair.corr;
        self.sum.carrier += pair.carrier;
        self.sum.step += pair.step;
        self.sum.current_power += pair.current_power;
        self.sum.delayed_power += pair.delayed_power;
        if self.pair_filled == self.pairs.len() {
            let denominator = (self.sum.current_power * self.sum.delayed_power)
                .max(0.0)
                .sqrt()
                .max(f32::EPSILON);
            let coherence = self.sum.corr.norm() / denominator;
            if coherence > self.best {
                self.best = coherence;
                self.best_phase = self.sum.corr.arg();
                self.best_carrier = self.sum.carrier.norm()
                    / (self.sum.current_power * self.pairs.len() as f32)
                        .sqrt()
                        .max(f32::EPSILON);
                self.best_tone = self.sum.step.norm() / self.pairs.len() as f32;
            }
        }
    }

    fn reset_measurement(&mut self) {
        self.best = 0.0;
        self.best_phase = 0.0;
        self.best_carrier = 0.0;
        self.best_tone = 0.0;
    }

    fn frequency_error_hz(&self, rate: f64) -> f32 {
        self.best_phase * rate as f32 / (2.0 * PI * self.useful as f32)
    }
}

pub struct DrmChannel {
    params: DrmParams,
    drm30: [CpProbe; 4],
    drm_plus: CpProbe,
    decimation: u8,
    samples: usize,
}

fn params(settings: &ChannelSettings) -> Result<DrmParams, ChannelError> {
    let ChannelParams::Drm(p) = settings.params else {
        return Err(ChannelError::InvalidSettings(format!(
            "drm channel got {} params",
            settings.params.type_id()
        )));
    };
    let valid = p.bandwidth_hz.is_finite()
        && match p.mode {
            DrmMode::Drm30 => {
                (MIN_DRM30_BANDWIDTH_HZ..=MAX_DRM30_BANDWIDTH_HZ).contains(&p.bandwidth_hz)
            }
            DrmMode::DrmPlus => (p.bandwidth_hz - DRM_PLUS_BANDWIDTH_HZ).abs() < 1.0,
            DrmMode::Auto => (p.bandwidth_hz - DRM_PLUS_BANDWIDTH_HZ).abs() < 1.0,
        };
    if valid {
        Ok(p)
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "DRM bandwidth {} Hz is invalid for {:?}",
            p.bandwidth_hz, p.mode
        )))
    }
}

pub(crate) fn occupied_band(p: &DrmParams) -> (f64, f64) {
    let bandwidth = match p.mode {
        DrmMode::Drm30 => p.bandwidth_hz,
        DrmMode::Auto | DrmMode::DrmPlus => DRM_PLUS_BANDWIDTH_HZ,
    };
    (-bandwidth / 2.0, bandwidth / 2.0)
}

pub(crate) fn channel_filter(p: &DrmParams) -> Result<ChannelFilter, ChannelError> {
    let p = params(&ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Drm(*p),
        audio: sdrmm_wire::AudioProcessing::default(),
    })?;
    let (_, half) = occupied_band(&p);
    let pass = half.min(flat_bandwidth_hz(INPUT_RATE_HZ) / 2.0);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(127, pass / INPUT_RATE_HZ),
        1,
    )))
}

impl DrmChannel {
    fn probes() -> [CpProbe; 4] {
        [
            CpProbe::new(1_152, 128),
            CpProbe::new(1_024, 256),
            CpProbe::new(704, 256),
            CpProbe::new(448, 352),
        ]
    }

    fn reset(&mut self) {
        self.drm30 = Self::probes();
        self.drm_plus = CpProbe::new(432, 48);
        self.decimation = 0;
        self.samples = 0;
    }

    fn report(&mut self, out: &mut ChannelOutputs) {
        let mut best_drm30 = &self.drm30[0];
        for probe in &self.drm30[1..] {
            if probe.best > best_drm30.best {
                best_drm30 = probe;
            }
        }
        let use_plus = match self.params.mode {
            DrmMode::Drm30 => false,
            DrmMode::DrmPlus => true,
            DrmMode::Auto => self.drm_plus.best > best_drm30.best,
        };
        let (system, coherence, carrier, tone, frequency_error_hz) = if use_plus {
            (
                BroadcastSystem::DrmPlus,
                self.drm_plus.best,
                self.drm_plus.best_carrier,
                self.drm_plus.best_tone,
                self.drm_plus.frequency_error_hz(INPUT_RATE_HZ),
            )
        } else {
            (
                BroadcastSystem::Drm30,
                best_drm30.best,
                best_drm30.best_carrier,
                best_drm30.best_tone,
                best_drm30.frequency_error_hz(DRM30_RATE_HZ),
            )
        };
        out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
            system,
            locked: coherence > 0.76 && carrier < 0.75 && tone < 0.995,
            snr_db: (-10.0 * (1.0 - coherence.clamp(0.0, 0.9999)).log10()).min(40.0),
            frequency_error_hz,
            ..BroadcastStatus::default()
        }));
        for probe in &mut self.drm30 {
            probe.reset_measurement();
        }
        self.drm_plus.reset_measurement();
        self.samples = 0;
    }
}

impl ChannelRx for DrmChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        Ok(Self {
            params: params(&settings)?,
            drm30: Self::probes(),
            drm_plus: CpProbe::new(432, 48),
            decimation: 0,
            samples: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.params = params(&settings)?;
        self.reset();
        Ok(())
    }

    fn retuned(&mut self) {
        self.reset();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for &sample in iq {
            if self.params.mode != DrmMode::Drm30 {
                self.drm_plus.push(sample);
            }
            if self.params.mode != DrmMode::DrmPlus && self.decimation == 0 {
                for probe in &mut self.drm30 {
                    probe.push(sample);
                }
            }
            self.decimation = (self.decimation + 1) % 4;
            self.samples += 1;
            if self.samples >= INPUT_RATE_HZ as usize {
                self.report(out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: DrmMode, bandwidth_hz: f64) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Drm(DrmParams { mode, bandwidth_hz }),
            audio: Default::default(),
        }
    }

    fn drm30_mode_b() -> Vec<Complex<f32>> {
        let mut baseband = Vec::new();
        let mut frame = 0usize;
        while baseband.len() < DRM30_RATE_HZ as usize {
            let mut state = 0x6d2b_79f5u32 ^ frame as u32;
            let carriers: Vec<_> = (-96i32..=96)
                .step_by(6)
                .filter(|&bin| bin != 0)
                .map(|bin| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    let symbol = match state & 3 {
                        0 => Complex::new(1.0, 1.0),
                        1 => Complex::new(-1.0, 1.0),
                        2 => Complex::new(-1.0, -1.0),
                        _ => Complex::new(1.0, -1.0),
                    };
                    (bin, symbol)
                })
                .collect();
            let scale = (carriers.len() as f32).sqrt();
            let useful: Vec<_> = (0..1_024)
                .map(|i| {
                    carriers
                        .iter()
                        .map(|&(bin, symbol)| {
                            symbol
                                * Complex::from_polar(
                                    1.0,
                                    2.0 * PI * bin as f32 * i as f32 / 1_024.0,
                                )
                        })
                        .sum::<Complex<f32>>()
                        / scale
                })
                .collect();
            baseband.extend_from_slice(&useful[768..]);
            baseband.extend_from_slice(&useful);
            frame += 1;
        }
        baseband.truncate(DRM30_RATE_HZ as usize);
        baseband
            .into_iter()
            .flat_map(|sample| std::iter::repeat_n(sample, 4))
            .collect()
    }

    fn drm_plus() -> Vec<Complex<f32>> {
        let mut baseband = Vec::new();
        let mut frame = 0usize;
        while baseband.len() < INPUT_RATE_HZ as usize {
            let mut state = 0x4f1b_cdcbu32 ^ frame as u32;
            let carriers: Vec<_> = (-108i32..=108)
                .step_by(6)
                .filter(|&bin| bin != 0)
                .map(|bin| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    let symbol = match state & 3 {
                        0 => Complex::new(1.0, 1.0),
                        1 => Complex::new(-1.0, 1.0),
                        2 => Complex::new(-1.0, -1.0),
                        _ => Complex::new(1.0, -1.0),
                    };
                    (bin, symbol)
                })
                .collect();
            let scale = (carriers.len() as f32).sqrt();
            let useful: Vec<_> = (0..432)
                .map(|i| {
                    carriers
                        .iter()
                        .map(|&(bin, symbol)| {
                            symbol
                                * Complex::from_polar(1.0, 2.0 * PI * bin as f32 * i as f32 / 432.0)
                        })
                        .sum::<Complex<f32>>()
                        / scale
                })
                .collect();
            baseband.extend_from_slice(&useful[384..]);
            baseband.extend_from_slice(&useful);
            frame += 1;
        }
        baseband.truncate(INPUT_RATE_HZ as usize);
        baseband
    }

    #[test]
    fn drm30_cyclic_prefix_acquires() {
        let mut channel = DrmChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(DrmMode::Drm30, 10_000.0),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        for chunk in drm30_mode_b().chunks(701) {
            channel.process(chunk, &mut out);
        }
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(status.locked, "{status:?}");
        assert_eq!(status.system, BroadcastSystem::Drm30);
        assert!(status.frequency_error_hz.abs() < 1.0);
    }

    #[test]
    fn drm_plus_cyclic_prefix_acquires_explicitly_and_automatically() {
        let iq = drm_plus();
        for mode in [DrmMode::DrmPlus, DrmMode::Auto] {
            let mut channel = DrmChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(mode, 100_000.0),
            )
            .unwrap();
            let mut out = ChannelOutputs::default();
            for chunk in iq.chunks(701) {
                channel.process(chunk, &mut out);
            }
            let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
                panic!("wrong event")
            };
            assert!(status.locked, "{status:?}");
            assert_eq!(status.system, BroadcastSystem::DrmPlus);
        }
    }

    #[test]
    fn noise_does_not_acquire() {
        let mut channel = DrmChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(DrmMode::Auto, 100_000.0),
        )
        .unwrap();
        let iq: Vec<_> = (0..INPUT_RATE_HZ as usize)
            .map(|i| {
                let a = ((i * 73) % 1_009) as f32 / 504.5 - 1.0;
                let b = ((i * 151 + 17) % 1_013) as f32 / 506.5 - 1.0;
                Complex::new(a, b)
            })
            .collect();
        let mut out = ChannelOutputs::default();
        channel.process(&iq, &mut out);
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(!status.locked, "{status:?}");
    }

    #[test]
    fn a_single_carrier_does_not_acquire() {
        let mut channel = DrmChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(DrmMode::Auto, 100_000.0),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        channel.process(
            &vec![Complex::new(1.0, 0.0); INPUT_RATE_HZ as usize],
            &mut out,
        );
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(!status.locked, "{status:?}");
    }

    #[test]
    fn acquisition_keeps_ahead_of_the_channel_rate() {
        let mut channel = DrmChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(DrmMode::Auto, 100_000.0),
        )
        .unwrap();
        let iq = vec![Complex::new(0.5, -0.5); INPUT_RATE_HZ as usize];
        let mut out = ChannelOutputs::default();
        let started = std::time::Instant::now();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(elapsed < 1.0, "one second of DRM took {elapsed:.2} s");
    }
}
