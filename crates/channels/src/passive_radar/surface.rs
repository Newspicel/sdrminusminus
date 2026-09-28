use sdrmm_wire::frame::{RangeDopplerOwned, SurfaceFrame};
use sdrmm_wire::radar::{SURFACE_DB_MAX, SURFACE_DB_MIN};

use super::plan::RadarPlan;
use super::report::RadarPod;

const POWER_FLOOR: f32 = 1e-6;
const LEVELS: f32 = 255.0;
const NANOS_PER_MILLI: u64 = 1_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct RangeDopplerSurface {
    pub seq: u64,
    pub unix_ms: u64,
    pub gates: usize,
    pub rows: usize,
    pub range_first_m: f32,
    pub range_step_m: f32,
    pub doppler_first_hz: f32,
    pub doppler_step_hz: f32,
    pub carrier_hz: f64,
    pub db_min: f32,
    pub db_max: f32,
    pub cells: Vec<u8>,
}

impl RangeDopplerSurface {
    #[must_use]
    pub fn new(plan: &RadarPlan) -> Self {
        let gates = plan.shape.gates;
        let rows = plan.report_rows.len();
        Self {
            seq: 0,
            unix_ms: 0,
            gates,
            rows,
            range_first_m: 0.0,
            range_step_m: plan.range_step_m as f32,
            doppler_first_hz: plan.doppler_of_row(plan.report_rows.start as f64) as f32,
            doppler_step_hz: plan.doppler_step_hz() as f32,
            carrier_hz: plan.carrier_hz,
            db_min: SURFACE_DB_MIN,
            db_max: SURFACE_DB_MAX,
            cells: vec![0; gates * rows],
        }
    }

    pub fn quantise(&mut self, power: &[f32], first_row: usize) {
        let start = first_row * self.gates;
        let source = power.get(start..start + self.cells.len()).unwrap_or(&[]);
        if source.len() != self.cells.len() {
            self.cells.fill(0);
            return;
        }
        for (cell, &value) in self.cells.iter_mut().zip(source) {
            *cell = level(value);
        }
    }

    #[must_use]
    pub fn decode(level: u8) -> f32 {
        SURFACE_DB_MIN + f32::from(level) / LEVELS * (SURFACE_DB_MAX - SURFACE_DB_MIN)
    }
}

#[must_use]
pub fn level(power: f32) -> u8 {
    let db = 10.0 * power.max(POWER_FLOOR).log10();
    let unit = ((db - SURFACE_DB_MIN) / (SURFACE_DB_MAX - SURFACE_DB_MIN)).clamp(0.0, 1.0);
    (LEVELS * unit).round() as u8
}

pub fn fill_surface(pod: &RadarPod, frame: &mut SurfaceFrame) {
    if !matches!(frame, SurfaceFrame::RangeDoppler(_)) {
        *frame = SurfaceFrame::RangeDoppler(RangeDopplerOwned::default());
    }
    let SurfaceFrame::RangeDoppler(owned) = frame else {
        return;
    };
    let surface = &pod.surface;
    owned.seq = pod.seq as u32;
    owned.timestamp = pod.cpi_end_unix_ns / NANOS_PER_MILLI;
    owned.ranges = u16::try_from(surface.gates).unwrap_or(u16::MAX);
    owned.dopplers = u16::try_from(surface.rows).unwrap_or(u16::MAX);
    owned.range_first_m = surface.range_first_m;
    owned.range_step_m = surface.range_step_m;
    owned.doppler_first_hz = surface.doppler_first_hz;
    owned.doppler_step_hz = surface.doppler_step_hz;
    owned.carrier_hz = surface.carrier_hz;
    owned.db_min = surface.db_min;
    owned.db_max = surface.db_max;
    owned.cells.clear();
    owned.cells.extend_from_slice(&surface.cells);
}
