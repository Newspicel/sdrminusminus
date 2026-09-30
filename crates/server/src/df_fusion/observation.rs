use sdrmm_wire::{BearingSource, DfBearing};

pub(crate) const RING: usize = 360;
pub(crate) const BANKS: usize = 8;
pub(crate) const BANK_WIDTHS_DEG: [f32; BANKS] = [0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];
pub(crate) const CORRELATION_S: f64 = 2.0;
pub(crate) const DEFAULT_ACCURACY_M: f32 = 10.0;
pub(crate) const FLOOR: f32 = 0.02;

const MIN_SIGMA_DEG: f32 = 0.5;
const MAX_SIGMA_DEG: f32 = 90.0;
const MIRROR_WEIGHT: f32 = 0.5;
const KERNEL_SIGMAS: f32 = 3.0;
const MIN_BLUR_DEG: f32 = 0.25;
const UNNAMED_STATION: &str = "station";

pub(crate) type Ring = [f32; RING];
pub(crate) type Rings = [Ring; BANKS];

pub(crate) struct Observation {
    pub(crate) station: String,
    pub(crate) at_s: f64,
    pub(crate) lat: f64,
    pub(crate) lon: f64,
    pub(crate) accuracy_m: f32,
    pub(crate) bearing_deg: f32,
    pub(crate) sigma_deg: f32,
    pub(crate) confidence: f32,
    pub(crate) heading_sigma_deg: f32,
    pub(crate) source: BearingSource,
    pub(crate) moving: bool,
    pub(crate) bank: Box<Rings>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    NoPosition,
    Weak,
    Unknown,
}

pub(crate) fn prepare(
    bearing: &DfBearing,
    at_s: f64,
    min_confidence: f32,
) -> Result<Observation, Refusal> {
    let (Some(lat), Some(lon)) = (bearing.lat, bearing.lon) else {
        return Err(Refusal::NoPosition);
    };
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        return Err(Refusal::NoPosition);
    }
    if !bearing.bearing_deg.is_finite()
        || bearing.confidence.is_nan()
        || bearing.confidence < min_confidence
    {
        return Err(Refusal::Weak);
    }
    let sigma_deg = clamp_sigma(bearing.sigma_deg);
    let heading_sigma_deg = heading_sigma_of(bearing);
    let bank = if bearing.likelihood.len() == RING {
        from_bytes(&bearing.likelihood)
    } else {
        synthesised(bearing, sigma_deg, heading_sigma_deg)
    };
    Ok(Observation {
        station: station_of(bearing),
        at_s,
        lat,
        lon,
        accuracy_m: bearing
            .accuracy_m
            .filter(|accuracy| accuracy.is_finite() && *accuracy >= 0.0)
            .unwrap_or(DEFAULT_ACCURACY_M),
        bearing_deg: bearing.bearing_deg.rem_euclid(360.0),
        sigma_deg,
        confidence: bearing.confidence.clamp(0.0, 1.0),
        heading_sigma_deg,
        source: bearing.source,
        moving: bearing.moving,
        bank,
    })
}

fn station_of(bearing: &DfBearing) -> String {
    bearing
        .station_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .or_else(|| Some(bearing.node.as_str()).filter(|node| !node.is_empty()))
        .unwrap_or(UNNAMED_STATION)
        .to_owned()
}

fn heading_sigma_of(bearing: &DfBearing) -> f32 {
    bearing
        .heading_sigma_deg
        .filter(|sigma| sigma.is_finite())
        .map_or(0.0, |sigma| sigma.clamp(0.0, MAX_SIGMA_DEG))
}

fn without_heading(sigma_deg: f32, heading_sigma_deg: f32) -> f32 {
    sigma_deg
        .mul_add(sigma_deg, -heading_sigma_deg * heading_sigma_deg)
        .max(MIN_SIGMA_DEG * MIN_SIGMA_DEG)
        .sqrt()
}

fn clamp_sigma(sigma_deg: f32) -> f32 {
    if sigma_deg.is_finite() {
        sigma_deg.clamp(MIN_SIGMA_DEG, MAX_SIGMA_DEG)
    } else {
        MAX_SIGMA_DEG
    }
}

pub(crate) fn wrap_deg(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(360.0);
    if wrapped > 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    }
}

#[derive(Clone, Copy)]
struct Peak {
    deg: f32,
    sigma: f32,
    weight: f32,
}

fn peaks(bearing: &DfBearing, sigma_deg: f32, heading_sigma_deg: f32) -> Vec<Peak> {
    let mirror = bearing.mirror_deg.filter(|mirror| mirror.is_finite());
    let main = if mirror.is_some() { MIRROR_WEIGHT } else { 1.0 };
    let own = without_heading(sigma_deg, heading_sigma_deg);
    let mut peaks = vec![Peak {
        deg: bearing.bearing_deg,
        sigma: own,
        weight: main,
    }];
    if let Some(mirror) = mirror {
        peaks.push(Peak {
            deg: mirror,
            sigma: own,
            weight: MIRROR_WEIGHT,
        });
    }
    peaks.extend(
        bearing
            .others
            .iter()
            .filter(|other| other.bearing_deg.is_finite() && other.confidence > 0.0)
            .map(|other| Peak {
                deg: other.bearing_deg,
                sigma: without_heading(clamp_sigma(other.sigma_deg), heading_sigma_deg),
                weight: other.confidence.min(1.0),
            }),
    );
    peaks
}

fn peak_sum(peaks: &[Peak], at_deg: f32, widening_deg: f32) -> f32 {
    peaks
        .iter()
        .map(|peak| {
            let spread2 = peak.sigma.mul_add(peak.sigma, widening_deg * widening_deg);
            let offset = wrap_deg(at_deg - peak.deg);
            peak.weight * (peak.sigma / spread2.sqrt()) * (-offset * offset / (2.0 * spread2)).exp()
        })
        .sum()
}

fn degree(index: usize) -> f32 {
    f32::from(u16::try_from(index).unwrap_or(u16::MAX))
}

fn synthesised(bearing: &DfBearing, sigma_deg: f32, heading_sigma_deg: f32) -> Box<Rings> {
    let peaks = peaks(bearing, sigma_deg, heading_sigma_deg);
    let top = (0..RING)
        .map(|index| peak_sum(&peaks, degree(index), 0.0))
        .fold(0.0f32, f32::max)
        .max(f32::MIN_POSITIVE);
    let mut bank = Box::new([[0.0; RING]; BANKS]);
    for (ring, widening) in bank.iter_mut().zip(BANK_WIDTHS_DEG) {
        for (index, value) in ring.iter_mut().enumerate() {
            let shape = peak_sum(&peaks, degree(index), widening) / top;
            *value = (1.0 - FLOOR).mul_add(shape, FLOOR).ln();
        }
    }
    bank
}

fn from_bytes(bytes: &[u8]) -> Box<Rings> {
    let floor = FLOOR.ln();
    let mut linear = [0.0f32; RING];
    for (value, &byte) in linear.iter_mut().zip(bytes) {
        *value = (floor - f32::from(byte) / 255.0 * floor).exp();
    }
    let mut bank = Box::new([[0.0; RING]; BANKS]);
    let mut widened = [0.0f32; RING];
    for (ring, widening) in bank.iter_mut().zip(BANK_WIDTHS_DEG) {
        blur(&linear, widening, &mut widened);
        for (value, &linear) in ring.iter_mut().zip(&widened) {
            *value = linear.max(f32::MIN_POSITIVE).ln();
        }
    }
    bank
}

pub(crate) fn blur(input: &Ring, sigma_deg: f32, out: &mut Ring) {
    if sigma_deg.is_nan() || sigma_deg < MIN_BLUR_DEG {
        *out = *input;
        return;
    }
    let reach = (KERNEL_SIGMAS * sigma_deg).ceil().clamp(1.0, 179.0);
    let taps = reach as usize;
    let kernel: Vec<f32> = (0..=2 * taps)
        .map(|tap| {
            let offset = degree(tap) - reach;
            (-offset * offset / (2.0 * sigma_deg * sigma_deg)).exp()
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    for (index, value) in out.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for (tap, weight) in kernel.iter().enumerate() {
            sum += weight * input[(index + RING + tap - taps) % RING];
        }
        *value = sum / total;
    }
}
