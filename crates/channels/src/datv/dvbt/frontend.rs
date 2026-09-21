use num_complex::Complex;
use sdrmm_wire::{BroadcastStatus, BroadcastSystem, DvbtParams, DvbtStandard};

use super::{receiver::Receiver, t2};
use crate::{ChannelError, datv::dvbs::PACKET};

pub enum Frontend {
    T(Box<Receiver>),
    T2(Box<t2::receiver::Receiver>),
}

impl Frontend {
    pub fn new(params: DvbtParams) -> Result<Self, ChannelError> {
        match params.standard {
            DvbtStandard::DvbT => Ok(Self::T(Box::new(Receiver::new(params.low_priority)))),
            DvbtStandard::DvbT2 => t2::receiver::Receiver::new(params.plp)
                .map(|r| Self::T2(Box::new(r)))
                .map_err(|error| ChannelError::InvalidSettings(error.to_string())),
        }
    }

    pub fn apply(&mut self, params: DvbtParams) -> Result<(), ChannelError> {
        match (self, params.standard) {
            (Self::T(r), DvbtStandard::DvbT) => {
                r.low_priority = params.low_priority;
                Ok(())
            }
            (Self::T2(r), DvbtStandard::DvbT2) => {
                r.select(params.plp);
                Ok(())
            }
            (this, _) => {
                *this = Self::new(params)?;
                Ok(())
            }
        }
    }

    pub fn reset(&mut self) {
        match self {
            Self::T(r) => r.reset(),
            Self::T2(r) => r.reset(),
        }
    }

    pub fn locked(&self) -> bool {
        match self {
            Self::T(r) => r.locked(),
            Self::T2(r) => r.locked(),
        }
    }

    pub fn push(&mut self, iq: &[Complex<f32>], packets: &mut Vec<[u8; PACKET]>) {
        match self {
            Self::T(r) => r.push(iq, packets),
            Self::T2(r) => r.push(iq, packets),
        }
    }

    pub fn status(&self, rate: f64) -> BroadcastStatus {
        match self {
            Self::T(r) => {
                let metrics = r.metrics();
                BroadcastStatus {
                    system: BroadcastSystem::DvbT,
                    locked: r.locked(),
                    snr_db: r.snr,
                    frequency_error_hz: r.frequency * rate as f32 / std::f32::consts::TAU,
                    code_rate: r.parameters.map(|p| {
                        format!(
                            "{}K {} {} 1/{}",
                            p.fft / 1024,
                            match p.bits {
                                2 => "QPSK",
                                4 => "16-QAM",
                                _ => "64-QAM",
                            },
                            if p.hierarchical && r.low_priority {
                                p.low_rate
                            } else {
                                p.high_rate
                            }
                            .label(),
                            p.fft / p.guard
                        )
                    }),
                    frames_ok: metrics.packets_ok,
                    frames_bad: metrics.packets_bad.saturating_add(r.bad_symbols),
                    bit_error_rate: metrics.byte_error_rate(),
                    ..BroadcastStatus::default()
                }
            }
            Self::T2(r) => {
                let metrics = r.report();
                BroadcastStatus {
                    system: BroadcastSystem::DvbT2,
                    locked: r.locked(),
                    snr_db: r.snr(),
                    frequency_error_hz: r.frequency * rate as f32 / std::f32::consts::TAU,
                    code_rate: r.plp().map(|p| {
                        format!(
                            "PLP {} {} {}",
                            p.id,
                            match p.coding.constellation {
                                t2::Constellation::Qpsk => "QPSK",
                                t2::Constellation::Qam16 => "16-QAM",
                                t2::Constellation::Qam64 => "64-QAM",
                                t2::Constellation::Qam256 => "256-QAM",
                            },
                            p.coding.rate.label()
                        )
                    }),
                    frames_ok: metrics.packets,
                    frames_bad: metrics.errors.saturating_add(r.errors),
                    data_error: r.last_error.or(metrics.last_error).map(|e| e.to_string()),
                    ..BroadcastStatus::default()
                }
            }
        }
    }
}
