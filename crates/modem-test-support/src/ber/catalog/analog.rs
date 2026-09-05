use sdrmm_modem::analog::{
    AmDetector, AmMode, AmParams, AmRx, AngleDetector, AngleKind, AngleParams, AngleRx, Sideband,
    SsbDetector, SsbMethod, SsbParams,
    am::{AmDemod, AmMod},
    angle::{AngleDemod, AngleMod},
    ssb::{SsbDemod, SsbMod},
};

use crate::ber::{
    analog::{AnalogLink, TonePlan},
    theory,
};

pub const WINDOW: usize = 8_192;

pub const SETTLE: usize = 4_096;

pub const TRIALS: usize = 3;

pub const SMOKE_POINTS: usize = 3;

pub const DRIVE: f32 = 1.0;

pub const TAPS: usize = 1_023;

pub const VOICE_RATE_HZ: f64 = 48_000.0;
pub const VOICE_BANDWIDTH: f64 = 3_000.0 / VOICE_RATE_HZ;
const VOICE_TONE: f64 = 1_000.0 / VOICE_RATE_HZ;

pub const WIDE_RATE_HZ: f64 = 240_000.0;
pub const WIDE_BANDWIDTH: f64 = 15_000.0 / WIDE_RATE_HZ;
const WIDE_TONE: f64 = 1_000.0 / WIDE_RATE_HZ;

pub const DEPTH: f64 = 0.8;

pub const NFM_DEVIATION_HZ: f64 = 2_500.0;
pub const WFM_DEVIATION_HZ: f64 = 75_000.0;
pub const PM_DEVIATION_RAD: f64 = 1.0;

pub const VSB_VESTIGE_HZ: f64 = 500.0;

fn voice_tone() -> TonePlan {
    TonePlan::new(VOICE_TONE, WINDOW)
}

fn wide_tone() -> TonePlan {
    TonePlan::new(WIDE_TONE, WINDOW)
}

#[must_use]
pub fn am_link(mode: AmMode, detector: AmDetector, label: &str) -> AnalogLink {
    am_link_at_taps(mode, detector, TAPS, label)
}

#[must_use]
pub fn am_link_at_taps(mode: AmMode, detector: AmDetector, taps: usize, label: &str) -> AnalogLink {
    let mut params = AmParams::new(mode, VOICE_BANDWIDTH);
    params.band_taps = taps;
    params.audio_taps = taps;
    AnalogLink {
        label: label.to_string(),
        bandwidth: VOICE_BANDWIDTH,
        tone: voice_tone(),
        drive: DRIVE,
        settle: SETTLE,
        modulate: Box::new(move |audio| {
            let mut out = Vec::new();
            AmMod::new(&params).process(audio, &mut out);
            out
        }),
        demodulate: Box::new(move |iq| {
            let mut out = Vec::new();
            AmDemod::new(&params, &AmRx::new(detector)).process(iq, &mut out);
            out
        }),
    }
}

#[must_use]
pub fn vsb_link() -> AnalogLink {
    let mut params = AmParams::new(AmMode::FullCarrier { depth: DEPTH }, VOICE_BANDWIDTH);
    params.vestige = Some(VSB_VESTIGE_HZ / VOICE_RATE_HZ);
    params.band_taps = TAPS;
    params.audio_taps = TAPS;
    let detector = AmDetector::Synchronous { loop_bw: 1e-3 };
    AnalogLink {
        label: format!(
            "VSB, {VSB_VESTIGE_HZ:.0} Hz vestige, depth {DEPTH}, synchronous, {VOICE_RATE_HZ:.0} Hz"
        ),
        bandwidth: VOICE_BANDWIDTH,
        tone: voice_tone(),
        drive: DRIVE,
        settle: SETTLE,
        modulate: Box::new(move |audio| {
            let mut out = Vec::new();
            AmMod::new(&params).process(audio, &mut out);
            out
        }),
        demodulate: Box::new(move |iq| {
            let mut out = Vec::new();
            AmDemod::new(&params, &AmRx::new(detector)).process(iq, &mut out);
            out
        }),
    }
}

#[must_use]
pub fn ssb_link(method: SsbMethod, detector: SsbDetector, label: &str) -> AnalogLink {
    let mut params = SsbParams::new(Sideband::Upper, method, VOICE_BANDWIDTH);
    params.band_taps = TAPS;
    params.audio_taps = TAPS;
    AnalogLink {
        label: label.to_string(),
        bandwidth: VOICE_BANDWIDTH,
        tone: voice_tone(),
        drive: DRIVE,
        settle: SETTLE,
        modulate: Box::new(move |audio| {
            let mut out = Vec::new();
            SsbMod::new(&params).process(audio, &mut out);
            out
        }),
        demodulate: Box::new(move |iq| {
            let mut out = Vec::new();
            SsbDemod::new(&params, detector, true).process(iq, &mut out);
            out
        }),
    }
}

#[must_use]
pub fn angle_link(
    params: AngleParams,
    tone: TonePlan,
    detector: AngleDetector,
    label: &str,
) -> AnalogLink {
    AnalogLink {
        label: label.to_string(),
        bandwidth: params.bandwidth,
        tone,
        drive: DRIVE,
        settle: SETTLE,
        modulate: Box::new(move |audio| {
            let mut out = Vec::new();
            AngleMod::new(&params).process(audio, &mut out);
            out
        }),
        demodulate: Box::new(move |iq| {
            let mut out = Vec::new();
            AngleDemod::new(&params, &AngleRx::new(detector)).process(iq, &mut out);
            out
        }),
    }
}

pub const FM_LOOP_BW: f64 = 2.0 * VOICE_BANDWIDTH;

pub const PM_LOOP_BW: f64 = 0.05 * VOICE_BANDWIDTH;

#[must_use]
pub fn am_envelope_link() -> AnalogLink {
    am_link(
        AmMode::FullCarrier { depth: DEPTH },
        AmDetector::Envelope,
        &format!("AM full carrier, depth {DEPTH}, envelope, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn am_synchronous_link() -> AnalogLink {
    am_link(
        AmMode::FullCarrier { depth: DEPTH },
        AmDetector::Synchronous { loop_bw: 1e-3 },
        &format!("AM full carrier, depth {DEPTH}, synchronous, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn dsb_link() -> AnalogLink {
    am_link(
        AmMode::Suppressed,
        AmDetector::Synchronous { loop_bw: 1e-3 },
        &format!("DSB-SC, synchronous, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn ssb_hilbert_link() -> AnalogLink {
    ssb_link(
        SsbMethod::Hilbert,
        SsbDetector::Filter,
        &format!("SSB USB, phasing exciter, filtering detector, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn ssb_weaver_link() -> AnalogLink {
    ssb_link(
        SsbMethod::Weaver,
        SsbDetector::Weaver,
        &format!("SSB USB, Weaver both ends, {VOICE_RATE_HZ:.0} Hz"),
    )
}

fn nfm_params() -> AngleParams {
    angle_params(
        AngleKind::Fm {
            deviation: NFM_DEVIATION_HZ / VOICE_RATE_HZ,
        },
        VOICE_BANDWIDTH,
    )
}

fn angle_params(kind: AngleKind, bandwidth: f64) -> AngleParams {
    let mut params = AngleParams::new(kind, bandwidth);
    params.band_taps = TAPS;
    params.audio_taps = TAPS;
    params
}

#[must_use]
pub fn nfm_discriminator_link() -> AnalogLink {
    angle_link(
        nfm_params(),
        voice_tone(),
        AngleDetector::Discriminator,
        &format!("NFM ±{NFM_DEVIATION_HZ:.0} Hz, discriminator, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn nfm_pll_link() -> AnalogLink {
    angle_link(
        nfm_params(),
        voice_tone(),
        AngleDetector::Pll {
            loop_bw: FM_LOOP_BW,
        },
        &format!("NFM ±{NFM_DEVIATION_HZ:.0} Hz, PLL, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn wfm_link() -> AnalogLink {
    angle_link(
        angle_params(
            AngleKind::Fm {
                deviation: WFM_DEVIATION_HZ / WIDE_RATE_HZ,
            },
            WIDE_BANDWIDTH,
        ),
        wide_tone(),
        AngleDetector::Discriminator,
        &format!("WFM ±{WFM_DEVIATION_HZ:.0} Hz, discriminator, {WIDE_RATE_HZ:.0} Hz"),
    )
}

#[must_use]
pub fn pm_link() -> AnalogLink {
    angle_link(
        angle_params(
            AngleKind::Pm {
                deviation_rad: PM_DEVIATION_RAD,
            },
            VOICE_BANDWIDTH,
        ),
        voice_tone(),
        AngleDetector::Discriminator,
        &format!("PM ±{PM_DEVIATION_RAD} rad, argument, {VOICE_RATE_HZ:.0} Hz"),
    )
}

#[derive(Clone, Copy, Debug)]
pub enum AnalogReference {
    Committed,
    Fom {
        name: &'static str,
        fom: f64,
        from_db: f64,
        tolerance_db: f64,
    },
}

impl AnalogReference {
    #[must_use]
    pub fn oracle(&self) -> Option<(&'static str, impl Fn(f64) -> f64 + use<>, f64, f64)> {
        match *self {
            Self::Committed => None,
            Self::Fom {
                name,
                fom,
                from_db,
                tolerance_db,
            } => Some((
                name,
                move |snr_db| theory::analog_sinad_db(fom, snr_db),
                from_db,
                tolerance_db,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AnalogMeasurement {
    pub stem: &'static str,
    pub link: fn() -> AnalogLink,
    pub grid: &'static [f64],
    pub seed: u64,
    pub trials: usize,
    pub smoke_points: usize,
    pub reference: AnalogReference,
}

impl AnalogMeasurement {
    #[must_use]
    pub fn tier(&self, full: bool) -> &'static [f64] {
        if full {
            self.grid
        } else {
            &self.grid[..self.smoke_points.min(self.grid.len())]
        }
    }

    #[must_use]
    pub fn artifact(&self) -> String {
        format!("{}/{}.json", super::BASELINE_DIR, self.stem)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AnalogEntry {
    pub name: &'static str,
    pub measurements: &'static [AnalogMeasurement],
}

pub const VOICE_GRID: &[f64] = &[0.0, 3.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0];

pub const WIDE_GRID: &[f64] = &[9.0, 12.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0, 33.0, 36.0];

pub const FOM_TOLERANCE_DB: f64 = 1.0;

const fn fom(name: &'static str, fom: f64, from_db: f64) -> AnalogReference {
    AnalogReference::Fom {
        name,
        fom,
        from_db,
        tolerance_db: FOM_TOLERANCE_DB,
    }
}

const fn measurement(
    stem: &'static str,
    link: fn() -> AnalogLink,
    grid: &'static [f64],
    seed: u64,
    reference: AnalogReference,
) -> AnalogMeasurement {
    AnalogMeasurement {
        stem,
        link,
        grid,
        seed,
        trials: TRIALS,
        smoke_points: SMOKE_POINTS,
        reference,
    }
}

const AM_FOM: f64 = 0.242_424_242_424_242_43;
const NFM_FOM: f64 = 1.041_666_666_666_666_7;
const WFM_FOM: f64 = 37.5;
const PM_FOM: f64 = 0.5;

const AM: &[AnalogMeasurement] = &[
    measurement(
        "analog/am_envelope_sinad",
        am_envelope_link,
        VOICE_GRID,
        0xa11e,
        fom("(m²P̄)/(1+m²P̄) at depth 0.8", AM_FOM, 21.0),
    ),
    measurement(
        "analog/am_synchronous_sinad",
        am_synchronous_link,
        VOICE_GRID,
        0xa115,
        fom("(m²P̄)/(1+m²P̄) at depth 0.8", AM_FOM, 21.0),
    ),
];

const DSB: &[AnalogMeasurement] = &[measurement(
    "analog/dsb_synchronous_sinad",
    dsb_link,
    VOICE_GRID,
    0xd5b0,
    fom("suppressed-carrier unity", 1.0, 12.0),
)];

const VSB: &[AnalogMeasurement] = &[measurement(
    "analog/vsb_synchronous_sinad",
    vsb_link,
    VOICE_GRID,
    0x5b0,
    AnalogReference::Committed,
)];

const SSB: &[AnalogMeasurement] = &[
    measurement(
        "analog/ssb_hilbert_sinad",
        ssb_hilbert_link,
        VOICE_GRID,
        0x55b1,
        fom("suppressed-carrier unity", 1.0, 12.0),
    ),
    measurement(
        "analog/ssb_weaver_sinad",
        ssb_weaver_link,
        VOICE_GRID,
        0x55b2,
        fom("suppressed-carrier unity", 1.0, 12.0),
    ),
];

const FM: &[AnalogMeasurement] = &[
    measurement(
        "analog/nfm_discriminator_sinad",
        nfm_discriminator_link,
        VOICE_GRID,
        0x8f30,
        fom("3β²P̄ at β = 0.833", NFM_FOM, 15.0),
    ),
    measurement(
        "analog/nfm_pll_sinad",
        nfm_pll_link,
        VOICE_GRID,
        0x8f31,
        AnalogReference::Committed,
    ),
    measurement(
        "analog/wfm_discriminator_sinad",
        wfm_link,
        WIDE_GRID,
        0x7f30,
        fom("3β²P̄ at β = 5", WFM_FOM, 24.0),
    ),
];

const PM: &[AnalogMeasurement] = &[measurement(
    "analog/pm_discriminator_sinad",
    pm_link,
    VOICE_GRID,
    0x9000,
    fom("β_p²P̄ at β_p = 1 rad", PM_FOM, 15.0),
)];

pub const ENTRIES: &[AnalogEntry] = &[
    AnalogEntry {
        name: "am",
        measurements: AM,
    },
    AnalogEntry {
        name: "dsb",
        measurements: DSB,
    },
    AnalogEntry {
        name: "vsb",
        measurements: VSB,
    },
    AnalogEntry {
        name: "ssb",
        measurements: SSB,
    },
    AnalogEntry {
        name: "fm",
        measurements: FM,
    },
    AnalogEntry {
        name: "pm",
        measurements: PM,
    },
];

#[must_use]
pub fn find(name: &str) -> Option<&'static AnalogEntry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

#[must_use]
pub fn measurement_for(stem: &str) -> Option<&'static AnalogMeasurement> {
    ENTRIES
        .iter()
        .flat_map(|entry| entry.measurements)
        .find(|m| m.stem == stem)
}

pub const AM_LIMITS: &str = "analog/am_envelope_limits";
pub const SSB_LIMITS: &str = "analog/ssb_hilbert_limits";
pub const NFM_LIMITS: &str = "analog/nfm_discriminator_limits";
pub const WFM_LIMITS: &str = "analog/wfm_discriminator_limits";

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_committed_figures_of_merit_are_their_closed_forms() {
        assert!((AM_FOM - theory::am_fom(DEPTH, 0.5)).abs() < 1e-15);
        assert!((NFM_FOM - theory::fm_fom(NFM_DEVIATION_HZ / 3_000.0, 0.5)).abs() < 1e-15);
        assert!((WFM_FOM - theory::fm_fom(WFM_DEVIATION_HZ / 15_000.0, 0.5)).abs() < 1e-15);
        assert!((PM_FOM - theory::pm_fom(PM_DEVIATION_RAD, 0.5)).abs() < 1e-15);
    }

    #[test]
    fn entry_names_and_artifact_stems_are_unique() {
        let mut names: Vec<&str> = ENTRIES.iter().map(|e| e.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate analog entry name");
        let mut stems: Vec<&str> = ENTRIES
            .iter()
            .flat_map(|e| e.measurements.iter().map(|m| m.stem))
            .collect();
        let count = stems.len();
        stems.sort_unstable();
        stems.dedup();
        assert_eq!(stems.len(), count, "duplicate analog artifact stem");
        for stem in &stems {
            assert!(super::super::measurement(stem).is_none(), "{stem}");
        }
    }

    #[test]
    fn every_registered_measurement_has_its_committed_artifact() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        for entry in ENTRIES {
            assert!(!entry.measurements.is_empty(), "entry `{}`", entry.name);
            for m in entry.measurements {
                assert!(
                    root.join(m.artifact()).is_file(),
                    "{}: {} missing",
                    entry.name,
                    m.artifact()
                );
                assert!(m.grid.windows(2).all(|w| w[1] > w[0]), "{}", m.stem);
                assert!(m.grid.len() >= SMOKE_POINTS, "{}", m.stem);
            }
        }
    }
}
