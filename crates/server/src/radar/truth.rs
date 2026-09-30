use std::collections::{HashMap, VecDeque};

use jiff::Timestamp;
use sdrmm_dsp::radar::{
    assign::Assignment,
    bistatic::{self, Geodetic},
};
use sdrmm_wire::{
    AdsbMatch, AdsbMessage, AdsbTruth, RadarAxes, RadarTrack,
    radar::{LIGHT_SPEED_M_S, MAX_RADAR_TRACKS, MAX_RADAR_TRUTH},
};

pub(crate) const MAX_AIRCRAFT: usize = 512;
const HISTORY: usize = 6;
const FEET_TO_M: f64 = 0.3048;
const KNOTS_TO_MPS: f64 = 0.514_444;
const FEET_PER_MINUTE_TO_MPS: f64 = 0.005_08;
const FORGET_AFTER_S: f64 = 60.0;
const STALE_AFTER_S: f64 = 30.0;
const EXTRAPOLATE_WITHIN_S: f64 = 10.0;
const ASSOCIATION_GATE: f64 = 16.0;

type Vec3 = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Baseline {
    pub(crate) rx: Geodetic,
    pub(crate) tx: Geodetic,
}

struct Frame {
    rx: Vec3,
    tx: Vec3,
    rx_site: Geodetic,
    wavelength_m: f64,
    reach_m: f64,
    doppler_max_hz: f64,
}

impl Frame {
    fn new(sites: Baseline, axes: &RadarAxes) -> Option<Self> {
        (axes.carrier_hz > 0.0).then(|| Self {
            rx: bistatic::ecef(sites.rx),
            tx: bistatic::ecef(sites.tx),
            rx_site: sites.rx,
            wavelength_m: LIGHT_SPEED_M_S / axes.carrier_hz,
            reach_m: f64::from(axes.gates) * f64::from(axes.range_step_m),
            doppler_max_hz: f64::from(axes.doppler_rows.saturating_sub(1)) / 2.0
                * f64::from(axes.doppler_step_hz),
        })
    }

    fn range_m(&self, target: Vec3) -> f64 {
        bistatic::bistatic_range_m(self.tx, self.rx, target)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Aircraft {
    pub(crate) icao: String,
    pub(crate) callsign: Option<String>,
    pub(crate) position: Option<(Geodetic, Timestamp)>,
    pub(crate) velocity: Option<(Vec3, Timestamp)>,
    history: VecDeque<(Timestamp, Geodetic)>,
    seen: Timestamp,
}

fn seconds_between(later: Timestamp, earlier: Timestamp) -> f64 {
    later.duration_since(earlier).as_secs_f64()
}

fn scaled(vector: Vec3, factor: f64) -> Vec3 {
    [vector[0] * factor, vector[1] * factor, vector[2] * factor]
}

fn median(values: &mut [f64]) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 1 => values.get(middle).copied(),
        _ => Some((values.get(middle - 1)? + values.get(middle)?) / 2.0),
    }
}

impl Aircraft {
    fn new(icao: &str, at: Timestamp) -> Self {
        Self {
            icao: icao.to_owned(),
            callsign: None,
            position: None,
            velocity: None,
            history: VecDeque::with_capacity(HISTORY),
            seen: at,
        }
    }

    fn merge(&mut self, at: Timestamp, message: &AdsbMessage) {
        self.seen = self.seen.max(at);
        if let Some(callsign) = message
            .callsign
            .as_deref()
            .map(str::trim)
            .filter(|callsign| !callsign.is_empty())
        {
            self.callsign = Some(callsign.to_owned());
        }
        self.place(at, message);
        if let (Some(speed_kt), Some(track_deg)) = (message.ground_speed_kt, message.track_deg) {
            let speed = speed_kt * KNOTS_TO_MPS;
            let (sin, cos) = track_deg.to_radians().sin_cos();
            let climb = message
                .vertical_rate_fpm
                .map_or(0.0, |rate| f64::from(rate) * FEET_PER_MINUTE_TO_MPS);
            self.velocity = Some(([speed * sin, speed * cos, climb], at));
        }
    }

    fn place(&mut self, at: Timestamp, message: &AdsbMessage) {
        let (Some(lat_deg), Some(lon_deg)) = (message.lat, message.lon) else {
            return;
        };
        let altitude = message
            .altitude_ft
            .map(|feet| f64::from(feet) * FEET_TO_M)
            .or_else(|| self.position.map(|(point, _)| point.alt_m));
        let Some(alt_m) = altitude else {
            return;
        };
        let point = Geodetic {
            lat_deg,
            lon_deg,
            alt_m,
        };
        self.position = Some((point, at));
        if self.history.back().is_some_and(|(last, _)| *last >= at) {
            return;
        }
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back((at, point));
    }

    fn fresh(&self, now: Timestamp) -> bool {
        let last = self.position.map_or(self.seen, |(_, at)| at);
        seconds_between(now, last) <= FORGET_AFTER_S
    }

    fn history_rate(&self, frame: &Frame) -> Option<f64> {
        let ranges: Vec<(Timestamp, f64)> = self
            .history
            .iter()
            .map(|(at, point)| (*at, frame.range_m(bistatic::ecef(*point))))
            .collect();
        let mut rates: Vec<f64> = ranges
            .windows(2)
            .filter_map(|pair| {
                let [(before, from), (after, to)] = pair else {
                    return None;
                };
                let elapsed = seconds_between(*after, *before);
                (elapsed > 0.0).then(|| (to - from) / elapsed)
            })
            .collect();
        median(&mut rates)
    }

    fn sighting(&self, now: Timestamp, frame: &Frame) -> Option<AdsbTruth> {
        let (point, at) = self.position?;
        let age_s = seconds_between(now, at);
        if age_s.abs() > STALE_AFTER_S {
            return None;
        }
        let velocity = self
            .velocity
            .filter(|(_, at)| seconds_between(now, *at).abs() <= STALE_AFTER_S)
            .map(|(velocity, _)| velocity);
        let target = match velocity {
            Some(velocity) if age_s.abs() <= EXTRAPOLATE_WITHIN_S => {
                bistatic::geodetic_from_enu(point, scaled(velocity, age_s))
            }
            _ => point,
        };
        let target_ecef = bistatic::ecef(target);
        let range_m = frame.range_m(target_ecef);
        let rate_mps = match velocity {
            Some(velocity) => bistatic::bistatic_rate_mps(
                frame.tx,
                frame.rx,
                target_ecef,
                bistatic::enu_vector_to_ecef(target, velocity),
            ),
            None => self.history_rate(frame)?,
        };
        let doppler_hz = -rate_mps / frame.wavelength_m;
        Some(AdsbTruth {
            icao: self.icao.clone(),
            callsign: self.callsign.clone(),
            range_km: (range_m / 1_000.0) as f32,
            doppler_hz: doppler_hz as f32,
            bearing_deg: bistatic::bearing_deg(frame.rx_site, target) as f32,
            lat: target.lat_deg,
            lon: target.lon_deg,
            altitude_m: target.alt_m as f32,
            age_s: age_s as f32,
            in_view: (0.0..=frame.reach_m).contains(&range_m)
                && doppler_hz.abs() <= frame.doppler_max_hz,
            track_id: None,
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct AircraftTable {
    aircraft: HashMap<String, Aircraft>,
}

impl AircraftTable {
    pub(crate) fn observe(&mut self, at: Timestamp, message: &AdsbMessage) {
        if message.icao.is_empty() {
            return;
        }
        if !self.aircraft.contains_key(&message.icao) && self.aircraft.len() >= MAX_AIRCRAFT {
            self.evict_oldest();
        }
        self.aircraft
            .entry(message.icao.clone())
            .or_insert_with(|| Aircraft::new(&message.icao, at))
            .merge(at, message);
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .aircraft
            .values()
            .min_by_key(|aircraft| aircraft.seen)
            .map(|aircraft| aircraft.icao.clone());
        if let Some(oldest) = oldest {
            self.aircraft.remove(&oldest);
        }
    }

    pub(crate) fn prune(&mut self, now: Timestamp) {
        self.aircraft.retain(|_, aircraft| aircraft.fresh(now));
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.aircraft.len()
    }

    #[cfg(test)]
    pub(crate) fn knows(&self, icao: &str) -> bool {
        self.aircraft.contains_key(icao)
    }

    pub(crate) fn truth(
        &self,
        now: Timestamp,
        sites: Baseline,
        axes: &RadarAxes,
        out: &mut Vec<AdsbTruth>,
    ) {
        out.clear();
        let Some(frame) = Frame::new(sites, axes) else {
            return;
        };
        out.extend(
            self.aircraft
                .values()
                .filter_map(|aircraft| aircraft.sighting(now, &frame)),
        );
        out.sort_by(|a, b| a.range_km.total_cmp(&b.range_km));
        out.truncate(MAX_RADAR_TRUTH);
    }
}

pub(crate) fn solver() -> Assignment {
    Assignment::new(MAX_RADAR_TRUTH, MAX_RADAR_TRACKS)
}

fn association_cost(truth: &AdsbTruth, track: &RadarTrack, axes: &RadarAxes) -> Option<f64> {
    let range_cell_m = 2.0 * f64::from(axes.range_step_m);
    let doppler_cell_hz = 2.0 * f64::from(axes.doppler_step_hz);
    if range_cell_m <= 0.0 || doppler_cell_hz <= 0.0 {
        return None;
    }
    let range = f64::from(truth.range_km - track.range_km) * 1_000.0 / range_cell_m;
    let doppler = f64::from(truth.doppler_hz - track.doppler_hz) / doppler_cell_hz;
    let cost = range * range + doppler * doppler;
    (cost <= ASSOCIATION_GATE).then_some(cost)
}

pub(crate) fn associate(
    solver: &mut Assignment,
    truth: &mut [AdsbTruth],
    tracks: &mut [RadarTrack],
    axes: &RadarAxes,
) {
    for sighting in truth.iter_mut() {
        sighting.track_id = None;
    }
    for track in tracks.iter_mut() {
        track.adsb = None;
    }
    let rows: Vec<usize> = truth
        .iter()
        .enumerate()
        .filter(|(_, sighting)| sighting.in_view)
        .map(|(index, _)| index)
        .take(MAX_RADAR_TRUTH)
        .collect();
    let cols = tracks.len().min(MAX_RADAR_TRACKS);
    let mut pairs = vec![None; rows.len()];
    let solved = solver.solve(
        rows.len(),
        cols,
        |row, col| association_cost(truth.get(*rows.get(row)?)?, tracks.get(col)?, axes),
        &mut pairs,
    );
    if let Err(error) = solved {
        tracing::warn!(%error, "ADS-B truth left unpaired");
        return;
    }
    for (row, col) in pairs.into_iter().enumerate() {
        let (Some(sighting), Some(track)) = (
            rows.get(row).and_then(|index| truth.get_mut(*index)),
            col.and_then(|col| tracks.get_mut(col)),
        ) else {
            continue;
        };
        sighting.track_id = Some(track.id);
        track.adsb = Some(AdsbMatch {
            icao: sighting.icao.clone(),
            callsign: sighting.callsign.clone(),
        });
    }
}
