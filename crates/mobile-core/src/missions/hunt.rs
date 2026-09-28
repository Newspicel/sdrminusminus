use sdrmm_wire::hunt::{HuntStatus, HuntSweep, SweepState};

use super::views::{HuntView, SweepPhase, SweepView, Trend};

pub(crate) const ON_TOP_STRENGTH: f32 = 0.9;

pub(crate) fn trend(status: &HuntStatus) -> Trend {
    if status.readings < 2 || status.smooth_db.is_none() {
        Trend::Waiting
    } else if status.closing {
        Trend::Warmer
    } else if status.strength >= ON_TOP_STRENGTH {
        Trend::OnTop
    } else {
        Trend::Colder
    }
}

pub(crate) fn project(mission: &str, status: &HuntStatus, running: bool) -> HuntView {
    HuntView {
        mission: mission.to_owned(),
        freq_hz: status.freq_hz,
        level_db: status.level_db,
        smooth_db: status.smooth_db,
        floor_db: status.floor_db,
        best_db: status.best_db,
        strength: if status.strength.is_finite() {
            status.strength.clamp(0.0, 1.0)
        } else {
            0.0
        },
        trend: trend(status),
        running: running && status.error.is_none(),
        refusal: status.error.clone(),
        readings: status.readings,
        sweep: status.sweep.as_ref().map(sweep),
    }
}

pub(crate) fn idle(
    mission: &str,
    freq_hz: f64,
    running: bool,
    refusal: Option<String>,
) -> HuntView {
    HuntView {
        mission: mission.to_owned(),
        freq_hz,
        level_db: None,
        smooth_db: None,
        floor_db: None,
        best_db: None,
        strength: 0.0,
        trend: Trend::Waiting,
        running: running && refusal.is_none(),
        refusal,
        readings: 0,
        sweep: None,
    }
}

fn sweep(sweep: &HuntSweep) -> SweepView {
    SweepView {
        bins: sweep.bins.clone(),
        peak_deg: sweep.peak_deg,
        covered_deg: sweep.covered_deg,
        phase: phase(sweep.state),
        sigma_deg: sweep.sigma_deg,
    }
}

const fn phase(state: SweepState) -> SweepPhase {
    match state {
        SweepState::Off => SweepPhase::Off,
        SweepState::Idle => SweepPhase::Idle,
        SweepState::Sweeping => SweepPhase::Sweeping,
        SweepState::NoHeading => SweepPhase::NoHeading,
        SweepState::ShortSpan => SweepPhase::ShortSpan,
        SweepState::LowContrast => SweepPhase::LowContrast,
        SweepState::PoorFit => SweepPhase::PoorFit,
        SweepState::HeadingPoor => SweepPhase::HeadingPoor,
        SweepState::TooFast => SweepPhase::TooFast,
        SweepState::Done => SweepPhase::Done,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use sdrmm_wire::hunt::HuntSettings;

    use super::*;

    pub(crate) fn status(channel: u32, readings: u64) -> HuntStatus {
        HuntStatus {
            settings: HuntSettings::for_channel(channel),
            freq_hz: 145.5e6,
            bw_hz: 12_500.0,
            level_db: Some(-60.0),
            smooth_db: Some(-61.0),
            floor_db: Some(-90.0),
            best_db: Some(-55.0),
            strength: 0.5,
            closing: false,
            readings,
            at_ms: 0,
            pose_drops: 0,
            sweep: None,
            error: None,
        }
    }

    #[test]
    fn trend_follows_the_web_rules() {
        assert_eq!(trend(&status(1, 1)), Trend::Waiting);
        let mut quiet = status(1, 5);
        quiet.smooth_db = None;
        assert_eq!(trend(&quiet), Trend::Waiting);
        let mut closing = status(1, 5);
        closing.closing = true;
        closing.strength = 0.95;
        assert_eq!(trend(&closing), Trend::Warmer);
        let mut strong = status(1, 5);
        strong.strength = 0.9;
        assert_eq!(trend(&strong), Trend::OnTop);
        assert_eq!(trend(&status(1, 5)), Trend::Colder);
    }

    #[test]
    fn a_refused_hunt_is_not_running_and_says_why() {
        let mut refused = status(3, 9);
        refused.error = Some("Scanning".to_owned());
        let view = project("hunt1", &refused, true);
        assert!(!view.running);
        assert_eq!(view.refusal.as_deref(), Some("Scanning"));
        assert!(project("hunt1", &status(3, 9), true).running);
        assert!(!project("hunt1", &status(3, 9), false).running);
        let mut odd = status(3, 9);
        odd.strength = f32::NAN;
        assert_eq!(project("hunt1", &odd, true).strength, 0.0);
    }

    #[test]
    fn a_sweep_is_carried_with_its_phase() {
        let mut sweeping = status(3, 9);
        sweeping.sweep = Some(HuntSweep {
            bins: vec![0, 10, 255],
            peak_deg: Some(47.0),
            covered_deg: 200.0,
            heading_deg: Some(10.0),
            state: SweepState::LowContrast,
            sigma_deg: Some(9.0),
            fit: None,
            contrast_db: None,
            rate_dps: 20.0,
            lag_ms: 5.0,
        });
        let view = project("hunt1", &sweeping, true);
        assert_eq!(
            view.sweep,
            Some(SweepView {
                bins: vec![0, 10, 255],
                peak_deg: Some(47.0),
                covered_deg: 200.0,
                phase: SweepPhase::LowContrast,
                sigma_deg: Some(9.0),
            })
        );
        assert_eq!(idle("hunt1", 1.0, false, None).trend, Trend::Waiting);
        assert!(idle("hunt1", 1.0, true, None).running);
        assert!(!idle("hunt1", 1.0, true, Some("Scanning".to_owned())).running);
    }
}
