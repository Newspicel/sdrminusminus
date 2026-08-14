//! The measured chains behind the analog `CATALOG.md` rows ( §7 phase 8), and the
//! registry `cargo xtask ber <entry>` dispatches on for them.
//!
//! Same contract as the BER registry next door — one definition of each chain, read by the
//! crate's own gates and by the command alike — with the substitution §5 item 4 makes: the
//! artifact is a [`SinadCurve`] against channel SNR rather than a `Curve` against Eb/N0, and the
//! reference is a **figure of merit** rather than an error-rate oracle.
//!
//! **Two geometries, ten rows.** Everything except broadcast FM is measured on one voice
//! geometry — 48 kHz, a 3 kHz message, a 1 kHz tone at full drive — so the ten rows differ by
//! their modulation and detector and by nothing else, and the figures of merit are directly
//! comparable across the table. Broadcast FM cannot be: 75 kHz of deviation does not fit in
//! 48 kHz, so the wideband row is measured at 240 kHz with a 15 kHz message, which is its own
//! standard's geometry.
//!
//! **Where an oracle applies and where it does not.** Seven rows are held to a closed form above
//! their detector's threshold. Three are commit-and-guard, each for a stated reason:
//!
//! - **VSB** — the complementary slope halves the carrier and re-weights the sidebands, so the
//!   fraction of transmitted power that is message depends on the slope's shape and not only on
//!   the depth. No closed form describes that; the measured curve is the reference, and the
//!   number that matters is its distance from the double-sideband row on the same depth.
//! - **The FM PLL tier** — a loop is a filter, and its closed-loop response shapes the parabolic
//!   output noise and the tone differently. The tier's value is below threshold anyway, and
//!   *that* is what its committed curve is compared with the discriminator's for.
//! - **The AM envelope tier below its knee** — held to the same oracle as the synchronous tier
//!   above threshold, since above threshold they are the same number; the knee itself is the
//!   committed quantity, and the two tiers' knees are the tier comparison.

use crate::{
    analog::{
        AmDetector, AmMode, AmParams, AmRx, AngleDetector, AngleKind, AngleParams, AngleRx,
        Sideband, SsbDetector, SsbMethod, SsbParams,
        am::{AmDemod, AmMod},
        angle::{AngleDemod, AngleMod},
        ssb::{SsbDemod, SsbMod},
    },
    ber::{
        analog::{AnalogLink, TonePlan},
        theory,
    },
};

// --- The measurement geometry -------------------------------------------------------------------

/// Samples one analysis window holds. Large enough that a point's SINAD is an average over
/// hundreds of tone cycles rather than a realisation — a voice-geometry window holds ~1000
/// independent noise samples, so its estimate's own scatter is ~0.13 dB — and a power of two so
/// the tone snaps to a bin at any of the rates below.
pub const WINDOW: usize = 8_192;

/// Samples discarded ahead of the window: filter group delay plus whatever a carrier loop needs
/// to acquire. Half a window, which is two and a half times the whole [`TAPS`] cascade's group
/// delay and past the slowest carrier loop's settling.
pub const SETTLE: usize = 4_096;

/// Realisations summed per committed point. The window already averages; the trials are what
/// keep a threshold-region point — where the errors are impulsive clicks rather than a steady
/// hiss — from being one burst's luck.
pub const TRIALS: usize = 3;

/// Grid points a smoke tier measures, from the front of the committed grid.
pub const SMOKE_POINTS: usize = 3;

/// Peak audio amplitude every row is driven at. Full scale, so the message power is `½` and the
/// figures of merit in [`theory`] are read at the `message_power` they are stated for.
pub const DRIVE: f32 = 1.0;

/// Taps in every filter of the measured configuration, and the one parameter here that is set
/// by the *acceptance* rather than by the waveform.
///
/// A figure of merit is stated for an ideal brick-wall receiver at the message bandwidth. A real
/// filter's `−6 dB` edge has a transition around it, and everything it removes inside the
/// message band is noise the chain does not have to carry — so a soft receiver measures
/// *better* than its own oracle, which the harness treats as a defect rather than a triumph
/// (`sweep`'s comparators). At `sdrmm_dsp`'s Blackman design the transition half-width is
/// `2.75/taps`, so this many taps put it at 4 % of a 3 kHz message and the residual bias inside
/// the rows' own tolerance. The consumers in `channels` run far shorter filters and are none the
/// worse for it: what a short filter costs is an acceptance against a closed form, not audio.
pub const TAPS: usize = 1_023;

/// The voice geometry: 48 kHz, a 3 kHz message, a 1 kHz test tone.
pub const VOICE_RATE_HZ: f64 = 48_000.0;
pub const VOICE_BANDWIDTH: f64 = 3_000.0 / VOICE_RATE_HZ;
const VOICE_TONE: f64 = 1_000.0 / VOICE_RATE_HZ;

/// The broadcast geometry: 240 kHz, a 15 kHz message, the same 1 kHz tone.
pub const WIDE_RATE_HZ: f64 = 240_000.0;
pub const WIDE_BANDWIDTH: f64 = 15_000.0 / WIDE_RATE_HZ;
const WIDE_TONE: f64 = 1_000.0 / WIDE_RATE_HZ;

/// Modulation depth the two full-carrier rows are measured at — the margin below 1 broadcast
/// practice leaves, so a peaking talker never folds the envelope through zero.
pub const DEPTH: f64 = 0.8;

/// Narrowband FM: ±2.5 kHz into a 3 kHz message, the 12.5 kHz channel plan's own deviation.
pub const NFM_DEVIATION_HZ: f64 = 2_500.0;
/// Broadcast FM: ±75 kHz into a 15 kHz message (ITU-R BS.450).
pub const WFM_DEVIATION_HZ: f64 = 75_000.0;
/// Peak phase deviation of the PM row, in radians. One radian is the value that makes the
/// 4.77 dB gap to FM at the same deviation ratio a bare reading of the two forms.
pub const PM_DEVIATION_RAD: f64 = 1.0;

/// Vestige of the VSB row: 500 Hz of the lower sideband kept, which is the shape a 3 kHz
/// message and a slope a sixth of it wide gives.
pub const VSB_VESTIGE_HZ: f64 = 500.0;

fn voice_tone() -> TonePlan {
    TonePlan::new(VOICE_TONE, WINDOW)
}

fn wide_tone() -> TonePlan {
    TonePlan::new(WIDE_TONE, WINDOW)
}

// --- The chains ---------------------------------------------------------------------------------

/// The two full-carrier AM rows and the suppressed-carrier one, at whichever detector.
#[must_use]
pub fn am_link(mode: AmMode, detector: AmDetector, label: &str) -> AnalogLink {
    am_link_at_taps(mode, detector, TAPS, label)
}

/// The same chain at an arbitrary filter length — what the gap-attribution measurement varies,
/// and the only reason [`TAPS`] is not simply baked in (see that constant's docs).
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

/// The vestigial-sideband row: the same engine with a slope carving the lower sideband away.
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

/// A single-sideband row: `method` builds the waveform, `detector` reads it — deliberately
/// crossed for the phasing row (see [`ssb`](crate::analog::ssb)).
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

/// An angle-modulation row at either geometry and either tier.
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

/// Loop bandwidth of the FM PLL tier, in cycles per sample: twice the message bandwidth, which
/// is as wide as a discrete second-order loop can usefully be run at this oversampling. The
/// consequence is measured rather than hidden — see the module docs and the tier's own row.
pub const FM_LOOP_BW: f64 = 2.0 * VOICE_BANDWIDTH;

/// Loop bandwidth of the PM tier: a twentieth of the message bandwidth, so the loop tracks the
/// carrier and *not* the modulation — the opposite requirement to the FM tier's, since here the
/// message is what the loop must leave in the phase error it reads out.
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

/// An angle-modulation parameterisation at the measured configuration's filter lengths.
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

// --- The registry -------------------------------------------------------------------------------

/// What a measured SINAD curve is judged against (§4.1, analog form).
#[derive(Clone, Copy, Debug)]
pub enum AnalogReference {
    /// Commit-and-guard: the committed artifact is the whole judgement.
    Committed,
    /// Held to a figure of merit — `SINAD = channel SNR + 10·log₁₀(fom)` — from `from_db`
    /// upward. The lower bound is not slack: below its detector's threshold no closed form
    /// describes an analog chain at all, and the knee is committed separately.
    Fom {
        name: &'static str,
        fom: f64,
        from_db: f64,
        tolerance_db: f64,
    },
}

impl AnalogReference {
    /// The oracle as a function of channel SNR, for the rows that have one.
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

/// One committed SINAD curve and everything needed to reproduce and judge it.
#[derive(Clone, Copy, Debug)]
pub struct AnalogMeasurement {
    /// Artifact stem under `baselines/`, without extension.
    pub stem: &'static str,
    pub link: fn() -> AnalogLink,
    pub grid: &'static [f64],
    pub seed: u64,
    pub trials: usize,
    pub smoke_points: usize,
    pub reference: AnalogReference,
}

impl AnalogMeasurement {
    /// This measurement's grid at one tier: the committed one, or its smoke prefix.
    #[must_use]
    pub fn tier(&self, full: bool) -> &'static [f64] {
        if full {
            self.grid
        } else {
            &self.grid[..self.smoke_points.min(self.grid.len())]
        }
    }

    /// Path of the committed artifact relative to the workspace root.
    #[must_use]
    pub fn artifact(&self) -> String {
        format!("{}/{}.json", super::BASELINE_DIR, self.stem)
    }
}

/// One analog catalog entry as the harness knows it — the name `cargo xtask ber <entry>` takes.
#[derive(Clone, Copy, Debug)]
pub struct AnalogEntry {
    pub name: &'static str,
    pub measurements: &'static [AnalogMeasurement],
}

/// The voice geometry's grid: 0 to 30 dB of channel SNR in 3 dB steps, which brackets every
/// voice row's threshold at the bottom and leaves four oracle points at the top.
pub const VOICE_GRID: &[f64] = &[0.0, 3.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0];

/// Broadcast FM's grid, shifted up: its predetection filter is twelve times its message
/// bandwidth, so the same channel SNR buys 10.8 dB less carrier-to-noise at the detector and
/// the threshold arrives that much later.
pub const WIDE_GRID: &[f64] = &[9.0, 12.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0, 33.0, 36.0];

/// Tolerance every figure-of-merit row is held to.
///
/// Wider than the linear engine's 0.2 dB, and the reason is measured rather than conceded: every
/// analog row sits 0.1 to 0.9 dB *above* its oracle, because a figure of merit is stated for a
/// brick-wall receiver at the message bandwidth and a real filter's transition throws away noise
/// inside that band which the ideal one keeps. The attribution is a measurement of its own
/// (`the_oracle_gap_is_the_receive_filters_own_transition`): sharpen the filters and the gap
/// shrinks toward zero, at the cost of a receiver nothing in `channels` would run.
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

/// `am_fom(0.8, ½)` — the fraction of a broadcast-depth carrier's power that is message.
const AM_FOM: f64 = 0.242_424_242_424_242_43;
/// `fm_fom(2.5/3, ½)`.
const NFM_FOM: f64 = 1.041_666_666_666_666_7;
/// `fm_fom(5, ½)`.
const WFM_FOM: f64 = 37.5;
/// `pm_fom(1, ½)`.
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

/// Every analog entry with a runner, in catalog order.
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

/// The entry registered under `name`, if any.
#[must_use]
pub fn find(name: &str) -> Option<&'static AnalogEntry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

/// The measurement that owns an artifact stem, whichever entry it belongs to.
#[must_use]
pub fn measurement_for(stem: &str) -> Option<&'static AnalogMeasurement> {
    ENTRIES
        .iter()
        .flat_map(|entry| entry.measurements)
        .find(|m| m.stem == stem)
}

/// Artifact stems of the committed limits tables (§4.3), one per detector tier that carries one.
pub const AM_LIMITS: &str = "analog/am_envelope_limits";
pub const SSB_LIMITS: &str = "analog/ssb_hilbert_limits";
pub const NFM_LIMITS: &str = "analog/nfm_discriminator_limits";
pub const WFM_LIMITS: &str = "analog/wfm_discriminator_limits";

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The four figure-of-merit constants the registry is built from are the closed forms they
    /// claim to be — a transcribed number that drifted from its formula would move every gate
    /// that reads it, silently and in the safe direction.
    #[test]
    fn the_committed_figures_of_merit_are_their_closed_forms() {
        assert!((AM_FOM - theory::am_fom(DEPTH, 0.5)).abs() < 1e-15);
        assert!((NFM_FOM - theory::fm_fom(NFM_DEVIATION_HZ / 3_000.0, 0.5)).abs() < 1e-15);
        assert!((WFM_FOM - theory::fm_fom(WFM_DEVIATION_HZ / 15_000.0, 0.5)).abs() < 1e-15);
        assert!((PM_FOM - theory::pm_fom(PM_DEVIATION_RAD, 0.5)).abs() < 1e-15);
    }

    /// Entry names are the command's public surface and stems are file paths: a duplicate of
    /// either silently shadows a measurement.
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
        // And no analog stem may collide with a BER one — they share a directory tree.
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
