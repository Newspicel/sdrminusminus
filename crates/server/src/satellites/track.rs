use sdrmm_engine::Doppler;
use sdrmm_orbit::{Observer, OrbitError, Pass, Satellite, downlink_hz, uplink_hz};
use sdrmm_wire::{PositionFix, SatelliteLook, SatelliteNode, SatellitePass};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Steering {
    pub(super) look: SatelliteLook,
    pub(super) doppler: Option<Doppler>,
    pub(super) uplink_hz: Option<f64>,
}

pub(super) fn observer(fix: &PositionFix) -> Observer {
    Observer {
        latitude_deg: fix.latitude,
        longitude_deg: fix.longitude,
        altitude_m: fix.altitude_m.unwrap_or(0.0),
    }
}

pub(super) fn steer(
    satellite: &Satellite,
    observer: &Observer,
    settings: &SatelliteNode,
    now: f64,
) -> Result<Steering, OrbitError> {
    let here = satellite.look(observer, now)?;
    let later = satellite.look(observer, now + 1.0)?;
    let doppler = settings.downlink_hz.map(|carrier| {
        let shift = |rate| downlink_hz(carrier, rate) - carrier;
        let shift_hz = shift(here.range_rate_km_s);
        Doppler {
            shift_hz,
            rate_hz_s: shift(later.range_rate_km_s) - shift_hz,
        }
    });
    Ok(Steering {
        look: SatelliteLook {
            azimuth_deg: here.azimuth_deg,
            elevation_deg: here.elevation_deg,
            range_km: here.range_km,
            range_rate_km_s: here.range_rate_km_s,
        },
        doppler,
        uplink_hz: settings
            .uplink_hz
            .map(|wanted| uplink_hz(wanted, here.range_rate_km_s)),
    })
}

pub(super) fn pass(pass: Pass) -> SatellitePass {
    let second = |unix: f64| unix.round() as i64;
    SatellitePass {
        aos: pass.aos_unix.map(second),
        los: pass.los_unix.map(second),
        max_elevation_deg: pass.max_elevation_deg,
        max_at: second(pass.max_unix),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_orbit::{Tle, unix_seconds};

    use super::*;

    const ISS: &str = "ISS (ZARYA)
1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999
2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.50377579432041";

    const BERLIN: Observer = Observer {
        latitude_deg: 52.52,
        longitude_deg: 13.405,
        altitude_m: 40.0,
    };

    fn iss() -> Satellite {
        Satellite::new(Tle::parse(ISS).expect("valid")).expect("usable")
    }

    #[test]
    fn a_pass_sweeps_the_doppler_from_high_to_low() {
        let iss = iss();
        let epoch = unix_seconds(iss.tle.epoch_jd);
        let pass = iss
            .next_pass(&BERLIN, epoch, 86_400.0, 0.0)
            .expect("propagates")
            .expect("passes");
        let settings = SatelliteNode {
            downlink_hz: Some(437_800_000.0),
            uplink_hz: Some(145_990_000.0),
            ..SatelliteNode::default()
        };
        let at = |t| steer(&iss, &BERLIN, &settings, t).expect("steers");
        let rising = at(pass.aos_unix.expect("rises") + 5.0);
        let setting = at(pass.los_unix.expect("sets") - 5.0);
        let (Some(up), Some(down)) = (rising.doppler, setting.doppler) else {
            panic!("a downlink always yields a correction");
        };
        assert!(
            up.shift_hz > 5_000.0 && down.shift_hz < -5_000.0,
            "{up:?} {down:?}"
        );
        assert!(up.rate_hz_s < 0.0 && down.rate_hz_s < 0.0);
        let uplink = rising.uplink_hz.expect("an uplink was set");
        assert!(
            uplink < 145_990_000.0,
            "an approaching satellite is sent low: {uplink}"
        );
    }

    #[test]
    fn without_a_downlink_only_the_look_is_known() {
        let iss = iss();
        let now = unix_seconds(iss.tle.epoch_jd);
        let steering = steer(&iss, &BERLIN, &SatelliteNode::default(), now).expect("steers");
        assert_eq!(steering.doppler, None);
        assert_eq!(steering.uplink_hz, None);
        assert!((-90.0..=90.0).contains(&steering.look.elevation_deg));
    }
}
