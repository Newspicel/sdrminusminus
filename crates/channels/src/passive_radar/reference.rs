use num_complex::Complex;
use sdrmm_dsp::radar::cma::ReferenceCma;
use sdrmm_wire::radar::{ReferenceCleaning, ReferenceHealth, ReferenceMode};

use super::dab_remod::DabRemod;
use crate::ChannelError;

type C32 = Complex<f32>;

pub enum ReferenceCleaner {
    Off,
    Cma(ReferenceCma),
    Dab(Box<DabRemod>),
}

impl ReferenceCleaner {
    pub fn new(cleaning: ReferenceCleaning, sample_rate: f64) -> Result<Self, ChannelError> {
        match cleaning {
            ReferenceCleaning::Off => Ok(Self::Off),
            ReferenceCleaning::Cma { taps, step } => {
                ReferenceCma::new(taps as usize, step, sample_rate)
                    .map(Self::Cma)
                    .map_err(|_| ChannelError::Refused("CMA taps out of range"))
            }
            ReferenceCleaning::DabRemod => Ok(Self::Dab(Box::new(DabRemod::new(sample_rate)?))),
        }
    }

    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        match self {
            Self::Off => {
                out.clear();
                out.extend_from_slice(input);
            }
            Self::Cma(cma) => {
                out.resize(input.len(), C32::default());
                cma.process(input, out);
            }
            Self::Dab(remod) => remod.process(input, out),
        }
    }

    #[must_use]
    pub fn health(&self) -> ReferenceHealth {
        match self {
            Self::Off => ReferenceHealth::default(),
            Self::Cma(cma) => ReferenceHealth {
                mode: ReferenceMode::Cma,
                locked: cma.locked(),
                quality_db: cma.gain_db(),
                fallback_frames: cma.fallback_frames(),
            },
            Self::Dab(remod) => remod.health(),
        }
    }

    #[must_use]
    pub fn latency(&self) -> usize {
        match self {
            Self::Off | Self::Cma(_) => 0,
            Self::Dab(remod) => remod.latency(),
        }
    }

    pub fn reset(&mut self) {
        match self {
            Self::Off => {}
            Self::Cma(cma) => cma.reset(),
            Self::Dab(remod) => remod.reset(),
        }
    }
}
