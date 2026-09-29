use sdrmm_dsp::radar::bistatic::{self, Geodetic};
use sdrmm_wire::{
    AdsbTruth, PositionFix, RadarAoa, RadarFix, RadarGeometry, RadarProblem, RadarSite, RadarTrack,
    RadarUpdate,
};

use super::truth;
use crate::array::ArrayPose;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Receiver {
    pub(crate) site: Geodetic,
    pub(crate) heading_deg: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sites {
    pub(crate) receiver: Option<Receiver>,
    pub(crate) transmitter: Option<Geodetic>,
}

fn site(point: Geodetic) -> RadarSite {
    RadarSite {
        lat: point.lat_deg,
        lon: point.lon_deg,
        altitude_m: point.alt_m,
    }
}

impl Sites {
    pub(crate) fn of(pose: Option<&ArrayPose>, transmitter: Option<&PositionFix>) -> Self {
        Self {
            receiver: pose.map(|pose| Receiver {
                site: Geodetic {
                    lat_deg: pose.lat,
                    lon_deg: pose.lon,
                    alt_m: pose.altitude_m.unwrap_or(0.0),
                },
                heading_deg: pose.heading_deg,
            }),
            transmitter: transmitter.map(|fix| Geodetic {
                lat_deg: fix.latitude,
                lon_deg: fix.longitude,
                alt_m: fix.altitude_m.unwrap_or(0.0),
            }),
        }
    }

    pub(crate) fn heading_deg(&self) -> Option<f64> {
        self.receiver.and_then(|receiver| receiver.heading_deg)
    }

    pub(crate) fn both(&self) -> Option<truth::Baseline> {
        Some(truth::Baseline {
            rx: self.receiver?.site,
            tx: self.transmitter?,
        })
    }

    pub(crate) fn problems(&self, out: &mut Vec<RadarProblem>) {
        if self.transmitter.is_none() {
            out.push(RadarProblem::NoTransmitter);
        }
        match self.receiver {
            None => out.push(RadarProblem::NoReceiver),
            Some(Receiver {
                heading_deg: None, ..
            }) => out.push(RadarProblem::NoHeading),
            Some(_) => {}
        }
    }

    pub(crate) fn geometry(&self) -> Option<RadarGeometry> {
        let sites = self.both()?;
        let [x, y, z] = bistatic::ecef(sites.tx);
        let [rx, ry, rz] = bistatic::ecef(sites.rx);
        let baseline_m = (x - rx).hypot(y - ry).hypot(z - rz);
        Some(RadarGeometry {
            receiver: site(sites.rx),
            transmitter: site(sites.tx),
            baseline_km: (baseline_m / 1_000.0) as f32,
            heading_deg: self.heading_deg().map(|heading| heading as f32),
        })
    }
}

fn turn(aoa: &mut RadarAoa, heading_deg: f64) {
    aoa.bearing_deg = Some((f64::from(aoa.azimuth_deg) + heading_deg).rem_euclid(360.0) as f32);
}

pub(crate) fn true_bearings(update: &mut RadarUpdate, heading_deg: f64) {
    for aoa in update
        .detections
        .iter_mut()
        .filter_map(|detection| detection.aoa.as_mut())
        .chain(
            update
                .tracks
                .iter_mut()
                .filter_map(|track| track.aoa.as_mut()),
        )
    {
        turn(aoa, heading_deg);
    }
}

fn target_altitude(track: &RadarTrack, truth: &[AdsbTruth], assumed_m: f32) -> (f32, bool) {
    truth
        .iter()
        .find(|sighting| sighting.track_id == Some(track.id))
        .map_or((assumed_m, false), |sighting| (sighting.altitude_m, true))
}

fn locate(track: &RadarTrack, sites: truth::Baseline, altitude: (f32, bool)) -> Option<RadarFix> {
    let aoa = track.aoa?;
    let bearing_deg = aoa.bearing_deg?;
    let located = bistatic::locate_with_sigma(
        sites.rx,
        sites.tx,
        f64::from(track.range_km) * 1_000.0,
        f64::from(bearing_deg),
        f64::from(altitude.0),
        f64::from(track.range_sigma_m),
        f64::from(aoa.sigma_deg),
    )?;
    Some(RadarFix {
        lat: located.point.lat_deg,
        lon: located.point.lon_deg,
        alt_m: altitude.0,
        alt_from_adsb: altitude.1,
        major_m: located.major_m as f32,
        minor_m: located.minor_m as f32,
        orientation_deg: located.orientation_deg as f32,
    })
}

pub(crate) fn fix_tracks(
    update: &mut RadarUpdate,
    sites: truth::Baseline,
    assumed_altitude_m: f32,
) {
    let RadarUpdate { tracks, truth, .. } = update;
    for track in tracks.iter_mut() {
        let altitude = target_altitude(track, truth, assumed_altitude_m);
        track.fix = locate(track, sites, altitude);
    }
}
