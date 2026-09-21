pub mod acquire;
pub mod bicm;
mod common;
pub mod equalize;
pub mod interleave;
pub mod mapping;
mod p1_tables;
mod pilot_tables;
pub mod receiver;
pub mod schedule;
pub mod signalling;
mod tables;
pub mod transport;
pub mod transport_clock;

pub use crate::datv::dvbs2::ldpc::{Frame, Rate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("Invalid DVB-T2 coding parameters")]
    Parameters,
    #[error("DVB-T2 signalling failed validation")]
    Signalling,
    #[error("DVB-T2 RF synchronization failed")]
    Acquisition,
    #[error("Selected DVB-T2 PLP is unavailable")]
    Plp,
    #[error("DVB-T2 stream discontinuity")]
    Discontinuity,
    #[error("DVB-T2 common PLP timing is unavailable")]
    CommonPlp,
    #[error("Invalid DVB-T2 block length")]
    Length,
    #[error("Non-finite DVB-T2 sample")]
    NonFinite,
    #[error("DVB-T2 LDPC decoding failed")]
    Ldpc,
    #[error("DVB-T2 BCH decoding failed")]
    Bch,
    #[error("DVB-T2 baseband header failed validation")]
    Header,
    #[error("DVB-T2 output buffer is full")]
    Capacity,
    #[error("DVB-T2 stream format is unsupported")]
    Stream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constellation {
    Qpsk,
    Qam16,
    Qam64,
    Qam256,
}

impl Constellation {
    pub const fn bits(self) -> usize {
        match self {
            Self::Qpsk => 2,
            Self::Qam16 => 4,
            Self::Qam64 => 6,
            Self::Qam256 => 8,
        }
    }

    pub fn rotation(self) -> f32 {
        match self {
            Self::Qpsk => 29.0_f32.to_radians(),
            Self::Qam16 => 16.8_f32.to_radians(),
            Self::Qam64 => 8.6_f32.to_radians(),
            Self::Qam256 => (1.0_f32 / 16.0).atan(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coding {
    pub frame: Frame,
    pub rate: Rate,
    pub constellation: Constellation,
    pub rotated: bool,
    pub lite: bool,
}

impl Coding {
    pub fn validate(self) -> Result<Self, DecodeError> {
        use Constellation::Qam256;
        use Rate::*;
        let rate_ok = if self.lite {
            self.frame == Frame::Short
                && matches!(self.rate, R1_3 | R2_5 | R1_2 | R3_5 | R2_3 | R3_4)
                && !(self.constellation == Qam256
                    && (self.rotated || matches!(self.rate, R2_3 | R3_4)))
        } else {
            matches!(self.frame, Frame::Short | Frame::Normal)
                && matches!(self.rate, R1_2 | R3_5 | R2_3 | R3_4 | R4_5 | R5_6)
        };
        if rate_ok {
            Ok(self)
        } else {
            Err(DecodeError::Parameters)
        }
    }

    pub const fn cells(self) -> usize {
        self.frame.length() / self.constellation.bits()
    }

    pub fn information(self) -> usize {
        self.rate.information(self.frame)
    }

    pub const fn correct(self) -> usize {
        if matches!(self.frame, Frame::Normal) && matches!(self.rate, Rate::R2_3 | Rate::R5_6) {
            10
        } else {
            12
        }
    }

    pub fn message(self) -> usize {
        let degree = if self.frame == Frame::Short { 14 } else { 16 };
        self.information() - degree * self.correct()
    }

    pub(super) fn addresses(self) -> Result<&'static [&'static [u16]], DecodeError> {
        Ok(match (self.frame, self.rate) {
            (Frame::Normal, Rate::R3_5) => &tables::NORMAL_R3_5,
            (Frame::Normal, Rate::R2_3) => &tables::NORMAL_R2_3,
            (Frame::Short, Rate::R3_5) => &tables::SHORT_R3_5,
            _ => self
                .rate
                .addresses(self.frame)
                .ok_or(DecodeError::Parameters)?,
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod rf_tests;
