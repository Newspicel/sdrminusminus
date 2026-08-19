use std::{f32::consts::PI, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    BroadcastStatus, BroadcastSystem, ChannelDescriptor, ChannelParams, ChannelSettings, DabMode,
    DabParams, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 2_048_000.0;
const BANDWIDTH_HZ: f64 = 1_536_000.0;
const NULL_MIN: usize = 2_100;
const NULL_MAX: usize = 3_200;
const SYMBOL_LEN: usize = 2_552;
const USEFUL_LEN: usize = 2_048;
const GUARD_LEN: usize = SYMBOL_LEN - USEFUL_LEN;
const REPORT_FRAMES: u32 = 10;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dab".to_owned(),
    name: "DAB / DAB+ acquisition".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

enum State {
    Search,
    Collect,
}

pub struct DabChannel {
    params: DabParams,
    state: State,
    average_power: f32,
    null_power: f32,
    null_samples: usize,
    symbol: Vec<Complex<f32>>,
    frames: u32,
    last_locked: bool,
}

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

impl DabChannel {
    fn reset(&mut self) {
        self.state = State::Search;
        self.null_samples = 0;
        self.null_power = 0.0;
        self.symbol.clear();
        self.frames = REPORT_FRAMES - 1;
        self.last_locked = false;
    }

    fn system(&self) -> BroadcastSystem {
        match self.params.mode {
            DabMode::DabPlus => BroadcastSystem::DabPlus,
            DabMode::Auto | DabMode::Dab => BroadcastSystem::Dab,
        }
    }

    fn finish_symbol(&mut self, out: &mut ChannelOutputs) {
        let mut corr = Complex::new(0.0f32, 0.0);
        let mut step = Complex::new(0.0f32, 0.0);
        let mut prefix_power = 0.0f32;
        let mut tail_power = 0.0f32;
        for i in 0..GUARD_LEN {
            let prefix = self.symbol[i];
            let tail = self.symbol[USEFUL_LEN + i];
            corr += prefix * tail.conj();
            prefix_power += prefix.norm_sqr();
            tail_power += tail.norm_sqr();
        }
        for pair in self.symbol.windows(2) {
            let denominator = (pair[0].norm() * pair[1].norm()).max(f32::EPSILON);
            step += pair[1] * pair[0].conj() / denominator;
        }
        let coherence = corr.norm() / (prefix_power * tail_power).sqrt().max(f32::EPSILON);
        let tone_coherence = step.norm() / (self.symbol.len() - 1) as f32;
        let locked = coherence > 0.72 && tone_coherence < 0.85;
        self.frames = self.frames.saturating_add(1);
        if locked != self.last_locked || self.frames >= REPORT_FRAMES {
            let null_mean = self.null_power / self.null_samples.max(1) as f32;
            let signal_to_null = self.average_power / null_mean.max(1e-12);
            let snr_db = (10.0 * signal_to_null.log10()).clamp(0.0, 60.0);
            let frequency_error_hz =
                corr.arg() * INPUT_RATE_HZ as f32 / (2.0 * PI * USEFUL_LEN as f32);
            out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
                system: self.system(),
                locked,
                snr_db,
                frequency_error_hz,
                symbol_rate: Some(1_000.0),
                ..BroadcastStatus::default()
            }));
            self.frames = 0;
        }
        self.last_locked = locked;
        self.state = State::Search;
        self.null_samples = 0;
        self.null_power = 0.0;
        self.symbol.clear();
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
            state: State::Search,
            average_power: 0.0,
            null_power: 0.0,
            null_samples: 0,
            symbol: Vec::with_capacity(SYMBOL_LEN),
            frames: REPORT_FRAMES - 1,
            last_locked: false,
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
            let power = sample.norm_sqr();
            if self.average_power == 0.0 {
                self.average_power = power;
            } else if !matches!(self.state, State::Search) || self.null_samples == 0 {
                self.average_power += 0.00002 * (power - self.average_power);
            }
            match self.state {
                State::Search => {
                    if power < self.average_power * 0.08 {
                        self.null_samples += 1;
                        self.null_power += power;
                    } else if (NULL_MIN..=NULL_MAX).contains(&self.null_samples) {
                        self.state = State::Collect;
                        self.symbol.push(sample);
                    } else {
                        self.null_samples = 0;
                        self.null_power = 0.0;
                    }
                }
                State::Collect => {
                    self.symbol.push(sample);
                    if self.symbol.len() == SYMBOL_LEN {
                        self.finish_symbol(out);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Dab(DabParams::default()),
            audio: Default::default(),
        }
    }

    fn frame() -> Vec<Complex<f32>> {
        let mut state = 0x5a17_91e3u32;
        let useful: Vec<_> = (0..USEFUL_LEN)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::from_polar(1.0, state as f32 * (2.0 * PI / u32::MAX as f32))
            })
            .collect();
        let mut iq = vec![Complex::new(0.0001, 0.0); 2_656];
        iq.extend_from_slice(&useful[USEFUL_LEN - GUARD_LEN..]);
        iq.extend_from_slice(&useful);
        iq
    }

    #[test]
    fn mode_i_null_and_cyclic_prefix_acquire() {
        let mut channel = DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        channel.process(&vec![Complex::new(1.0, 0.0); 8_000], &mut out);
        for chunk in frame().chunks(137) {
            channel.process(chunk, &mut out);
        }
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(status.locked);
        assert_eq!(status.system, BroadcastSystem::Dab);
        assert!(status.frequency_error_hz.abs() < 1.0);
    }

    #[test]
    fn configured_dab_plus_generation_is_reported() {
        let mut configured = settings();
        configured.params = ChannelParams::Dab(DabParams {
            mode: DabMode::DabPlus,
            ..DabParams::default()
        });
        let mut channel = DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            configured,
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        channel.process(&vec![Complex::new(1.0, 0.0); 8_000], &mut out);
        channel.process(&frame(), &mut out);
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(status.locked);
        assert_eq!(status.system, BroadcastSystem::DabPlus);
    }

    #[test]
    fn a_power_drop_without_a_cyclic_prefix_does_not_lock() {
        let mut channel = DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(),
        )
        .unwrap();
        let mut iq = vec![Complex::new(1.0, 0.0); 8_000];
        iq.extend(vec![Complex::new(0.0001, 0.0); 2_656]);
        let mut state = 0x1234_5678u32;
        iq.extend((0..SYMBOL_LEN).map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            Complex::from_polar(1.0, state as f32 * (2.0 * PI / u32::MAX as f32))
        }));
        let mut out = ChannelOutputs::default();
        channel.process(&iq, &mut out);
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(!status.locked);
    }

    #[test]
    fn a_power_drop_followed_by_a_carrier_does_not_lock() {
        let mut channel = DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(),
        )
        .unwrap();
        let mut iq = vec![Complex::new(1.0, 0.0); 8_000];
        iq.extend(vec![Complex::new(0.0001, 0.0); 2_656]);
        iq.extend(vec![Complex::new(1.0, 0.0); SYMBOL_LEN]);
        let mut out = ChannelOutputs::default();
        channel.process(&iq, &mut out);
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(!status.locked);
    }

    #[test]
    fn acquisition_keeps_ahead_of_the_channel_rate() {
        let mut channel = DabChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(),
        )
        .unwrap();
        let iq = vec![Complex::new(0.5, -0.5); INPUT_RATE_HZ as usize];
        let mut out = ChannelOutputs::default();
        let started = std::time::Instant::now();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(elapsed < 1.0, "one second of DAB took {elapsed:.2} s");
    }
}
