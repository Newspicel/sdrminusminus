use std::{array, f32::consts::PI, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass, flat_bandwidth_hz};
use sdrmm_wire::{
    BroadcastStatus, BroadcastSystem, ChannelDescriptor, ChannelParams, ChannelSettings,
    DatvParams, DatvStandard, DecoderEvent,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 2_000_000.0;
const BANDWIDTH_HZ: f64 = 1_500_000.0;
const MIN_SYMBOL_RATE: f64 = 100_000.0;
const MAX_SYMBOL_RATE: f64 = 1_000_000.0;
const PHASE_BINS: usize = 32;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "datv".to_owned(),
    name: "DATV (DVB-S / S2) acquisition".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Clone, Copy, Default)]
struct Bin {
    fourth: Complex<f32>,
    eighth: Complex<f32>,
    carrier: Complex<f32>,
    fourth_step: Complex<f32>,
    eighth_step: Complex<f32>,
    previous_fourth: Option<Complex<f32>>,
    previous_eighth: Option<Complex<f32>>,
    power: f64,
    count: u32,
    octants: u8,
}

pub struct DatvChannel {
    params: DatvParams,
    phase: f64,
    bins: [Bin; PHASE_BINS],
    samples: usize,
}

fn params(settings: &ChannelSettings) -> Result<DatvParams, ChannelError> {
    match settings.params {
        ChannelParams::Datv(p) => {
            if p.symbol_rate.is_finite()
                && (MIN_SYMBOL_RATE..=MAX_SYMBOL_RATE).contains(&p.symbol_rate)
            {
                Ok(p)
            } else {
                Err(ChannelError::InvalidSettings(format!(
                    "DATV symbol rate must be in [{MIN_SYMBOL_RATE}, {MAX_SYMBOL_RATE}] baud, got {}",
                    p.symbol_rate
                )))
            }
        }
        ref other => Err(ChannelError::InvalidSettings(format!(
            "datv channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band(p: &DatvParams) -> (f64, f64) {
    let half = (p.symbol_rate * 1.35 / 2.0).min(BANDWIDTH_HZ / 2.0);
    (-half, half)
}

pub(crate) fn channel_filter(p: &DatvParams) -> Result<ChannelFilter, ChannelError> {
    let p = params(&ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        params: ChannelParams::Datv(*p),
    })?;
    let (_, half) = occupied_band(&p);
    let pass = half.min(flat_bandwidth_hz(INPUT_RATE_HZ) / 2.0);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(127, pass / INPUT_RATE_HZ),
        1,
    )))
}

impl DatvChannel {
    fn clear_measurement(&mut self) {
        self.bins = array::from_fn(|_| Bin::default());
        self.samples = 0;
    }

    fn system(&self) -> BroadcastSystem {
        match self.params.standard {
            DatvStandard::DvbS => BroadcastSystem::DvbS,
            DatvStandard::DvbS2 => BroadcastSystem::DvbS2,
        }
    }

    fn report(&mut self, out: &mut ChannelOutputs) {
        let (best_index, order, coherence) = self
            .bins
            .iter()
            .enumerate()
            .flat_map(|(index, bin)| {
                let count = bin.count.max(1) as f32;
                [
                    (index, 4u8, bin.fourth.norm() / count),
                    (index, 8u8, bin.eighth.norm() / count),
                ]
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .unwrap_or((0, 4, 0.0));
        let best = self.bins[best_index];
        let count = best.count.max(1) as f32;
        let carrier = best.carrier.norm() / count;
        let occupied = best.octants.count_ones();
        let locked = coherence > 0.68 && carrier < 0.75 && occupied >= 3 && best.power > 1e-8;
        let step = if order == 4 {
            best.fourth_step
        } else {
            best.eighth_step
        };
        let frequency_error_hz =
            step.arg() * self.params.symbol_rate as f32 / (2.0 * PI * f32::from(order));
        let snr_db = (-10.0 * (1.0 - coherence.clamp(0.0, 0.9999)).log10()).min(40.0);
        out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
            system: self.system(),
            locked,
            snr_db,
            frequency_error_hz,
            symbol_rate: Some(self.params.symbol_rate),
            ..BroadcastStatus::default()
        }));
        self.clear_measurement();
    }
}

impl ChannelRx for DatvChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        Ok(Self {
            params: params(&settings)?,
            phase: 0.0,
            bins: array::from_fn(|_| Bin::default()),
            samples: 0,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.params = params(&settings)?;
        self.phase = 0.0;
        self.clear_measurement();
        Ok(())
    }

    fn retuned(&mut self) {
        self.phase = 0.0;
        self.clear_measurement();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let step = self.params.symbol_rate / INPUT_RATE_HZ;
        for &sample in iq {
            let norm = sample.norm();
            if norm > 1e-6 && norm.is_finite() {
                let unit = sample / norm;
                let fourth = unit * unit * unit * unit;
                let eighth = fourth * fourth;
                let index = ((self.phase * PHASE_BINS as f64) as usize).min(PHASE_BINS - 1);
                let bin = &mut self.bins[index];
                bin.fourth += fourth;
                bin.eighth += eighth;
                bin.carrier += unit;
                if let Some(previous) = bin.previous_fourth {
                    bin.fourth_step += fourth * previous.conj();
                }
                if let Some(previous) = bin.previous_eighth {
                    bin.eighth_step += eighth * previous.conj();
                }
                bin.previous_fourth = Some(fourth);
                bin.previous_eighth = Some(eighth);
                bin.power += f64::from(sample.norm_sqr());
                bin.count = bin.count.saturating_add(1);
                let octant = ((unit.arg() + PI) * (4.0 / PI)).floor() as i32;
                bin.octants |= 1 << octant.rem_euclid(8);
            }
            self.phase += step;
            self.phase -= self.phase.floor();
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

    fn settings(symbol_rate: f64, standard: DatvStandard) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: ChannelParams::Datv(DatvParams {
                standard,
                symbol_rate,
            }),
        }
    }

    #[test]
    fn qpsk_carrier_acquires_at_the_configured_symbol_rate() {
        let symbol_rate = 250_000.0;
        let points = [
            Complex::new(1.0, 1.0),
            Complex::new(-1.0, 1.0),
            Complex::new(-1.0, -1.0),
            Complex::new(1.0, -1.0),
        ];
        let mut iq = Vec::with_capacity(INPUT_RATE_HZ as usize);
        for i in 0..(INPUT_RATE_HZ as usize / 8) {
            iq.extend(std::iter::repeat_n(points[(i * 13 + i / 7) % 4], 8));
        }
        for (standard, system) in [
            (DatvStandard::DvbS, BroadcastSystem::DvbS),
            (DatvStandard::DvbS2, BroadcastSystem::DvbS2),
        ] {
            let mut channel = DatvChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(symbol_rate, standard),
            )
            .unwrap();
            let mut out = ChannelOutputs::default();
            for chunk in iq.chunks(997) {
                channel.process(chunk, &mut out);
            }
            let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
                panic!("wrong event")
            };
            assert!(status.locked, "{status:?}");
            assert_eq!(status.system, system);
        }
    }

    #[test]
    fn eight_psk_carrier_acquires_for_dvb_s2() {
        let symbol_rate = 250_000.0;
        let points: Vec<_> = (0..8)
            .map(|i| Complex::from_polar(1.0, i as f32 * PI / 4.0))
            .collect();
        let mut iq = Vec::with_capacity(INPUT_RATE_HZ as usize);
        for i in 0..(INPUT_RATE_HZ as usize / 8) {
            iq.extend(std::iter::repeat_n(points[(i * 13 + i / 7) % 8], 8));
        }
        let mut channel = DatvChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(symbol_rate, DatvStandard::DvbS2),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        channel.process(&iq, &mut out);
        let DecoderEvent::Broadcast(status) = out.events.last().unwrap() else {
            panic!("wrong event")
        };
        assert!(status.locked, "{status:?}");
    }

    #[test]
    fn a_single_unmodulated_carrier_is_not_datv() {
        let mut channel = DatvChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(250_000.0, DatvStandard::DvbS),
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
        assert!(!status.locked);
    }

    #[test]
    fn acquisition_keeps_ahead_of_the_channel_rate() {
        let mut channel = DatvChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(250_000.0, DatvStandard::DvbS2),
        )
        .unwrap();
        let iq = vec![Complex::new(1.0, 1.0); INPUT_RATE_HZ as usize];
        let mut out = ChannelOutputs::default();
        let started = std::time::Instant::now();
        for block in iq.chunks(16_384) {
            channel.process(block, &mut out);
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(elapsed < 1.0, "one second of DATV took {elapsed:.2} s");
    }
}
