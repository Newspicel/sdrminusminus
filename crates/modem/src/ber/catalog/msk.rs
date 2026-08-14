use super::{
    Measurement,
    framing::{Acquisition, SPS, steady_link},
};
use crate::{
    ber::sweep::Link,
    cpm::{CpmParams, Mapping},
    pulse::{self, Norm},
};

#[must_use]
pub fn params() -> CpmParams {
    CpmParams::from_h(Mapping::natural(2), 0.5, pulse::rect(SPS, Norm::Area), SPS)
}

/// The integrate-and-dump receive filter — this pulse's matched filter.
#[must_use]
pub fn rx() -> Vec<f32> {
    pulse::rect(SPS, Norm::Area)
}

#[must_use]
pub fn link() -> Link {
    steady_link(
        "msk (1REC h=0.5) uncoded, CpmMod -> +/-6 kHz front lowpass -> CpmDemod \
         (integrate-and-dump rx, timing bw 0.015), 48 kHz 4800 baud, 96+24+24 symbol \
         overhead in Eb, release",
        Acquisition::Alternating,
        params(),
        rx(),
    )
}

pub const GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
pub const SEED: u64 = 0x635b;
pub const AWGN: &str = "cpm/msk_awgn";
pub const LIMITS: &str = "cpm/msk_limits";
pub const PERF: &str = "cpm/msk_perf";

pub const MEASUREMENTS: &[Measurement] = &[Measurement::committed(
    AWGN,
    link,
    GRID,
    SEED,
    super::framing::FULL_CAP,
)];
