use sdrmm_dsp::doa::{
    ANTENNA_PHASE_SIGMA_DEG, DEFAULT_CAL_SIGMA_DEG, DoaError, DoaReport, LIKELIHOOD_POINTS,
    MEASURED_PHASE_SIGMA_DEG, Peak, quantize_likelihood,
};
use sdrmm_dsp::manifold::AzimuthSpan;
use sdrmm_dsp::special::norm_deg;
use sdrmm_wire::processor::df::{DF_POINTS, MAX_DF_PEAKS, MAX_STATION_ID_LEN};
use sdrmm_wire::{
    BearingSource, CalPhase, CalSourceKind, DecoderEvent, DfAlgorithm, DfBearing, DfOtherPeak,
    DfPeak, DfReading, LatLon, ProcessorReading,
};

use super::DfProcessor;
use crate::array_processor::{GeoFix, ProcessorFaults, ProcessorOutput, Steer, stamp_at};

const GRID_SPAN_DB: f32 = 30.0;
const SUBSPACE_SPAN_DB: f32 = 40.0;
const MAX_OTHERS: usize = 3;
const MIN_POWER: f32 = 1e-30;
const NANOS_PER_MILLI: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Outcome {
    heading_deg: Option<f64>,
    heading_sigma_deg: f64,
    likelihood: bool,
    singular: bool,
    estimated: bool,
}

impl Outcome {
    fn true_deg(&self, relative_deg: f64) -> Option<f64> {
        self.heading_deg
            .map(|heading| norm_deg(relative_deg + heading))
    }

    fn event_sigma(&self, sigma_deg: f32) -> f32 {
        f64::from(sigma_deg).hypot(self.heading_sigma_deg) as f32
    }
}

const fn span_db(algorithm: DfAlgorithm) -> f32 {
    match algorithm {
        DfAlgorithm::Bartlett | DfAlgorithm::Capon => GRID_SPAN_DB,
        DfAlgorithm::Music | DfAlgorithm::RootMusic | DfAlgorithm::Esprit => SUBSPACE_SPAN_DB,
    }
}

fn decibels(power: f32) -> f32 {
    10.0 * power.max(MIN_POWER).log10()
}

fn level(value: f32, max: f32, span: f32) -> u8 {
    if value <= 0.0 || max <= 0.0 || !value.is_finite() || !max.is_finite() {
        return 0;
    }
    let db = 10.0 * (value / max).log10();
    let byte = (255.0 * (db + span) / span).round();
    if byte.is_nan() {
        0
    } else {
        byte.clamp(0.0, 255.0) as u8
    }
}

fn extend_capped<T: Copy>(faults: &mut ProcessorFaults, list: &mut Vec<T>, items: &[T]) {
    let room = list.capacity().saturating_sub(list.len());
    let take = items.len().min(room);
    list.extend_from_slice(&items[..take]);
    if take < items.len() {
        faults.truncated += 1;
    }
}

fn write_text(target: &mut String, text: &str) {
    target.clear();
    target.push_str(text);
}

fn reserved_bearing(node_len: usize) -> DfBearing {
    DfBearing {
        bearing_deg: 0.0,
        confidence: 0.0,
        lat: None,
        lon: None,
        station_id: Some(String::with_capacity(MAX_STATION_ID_LEN.max(node_len))),
        node: String::with_capacity(node_len),
        sigma_deg: 0.0,
        accuracy_m: None,
        heading_deg: None,
        heading_sigma_deg: None,
        relative_deg: None,
        mirror_deg: None,
        freq_hz: None,
        source: BearingSource::Array,
        moving: false,
        others: Vec::with_capacity(MAX_OTHERS),
        likelihood: Vec::with_capacity(DF_POINTS),
    }
}

fn rotate(relative: &[f32; LIKELIHOOD_POINTS], heading_deg: f64, out: &mut [f32]) {
    let points = LIKELIHOOD_POINTS as f64;
    for (degree, value) in out.iter_mut().enumerate().take(LIKELIHOOD_POINTS) {
        let at = (degree as f64 - heading_deg).rem_euclid(points);
        let low = (at.floor() as usize) % LIKELIHOOD_POINTS;
        let high = (low + 1) % LIKELIHOOD_POINTS;
        let fraction = (at - at.floor()) as f32;
        *value = relative[low] + (relative[high] - relative[low]) * fraction;
    }
}

impl DfProcessor {
    pub(super) fn publish(&mut self, out: &mut ProcessorOutput<'_>) {
        let outcome = self.estimate_now();
        self.write_reading(&outcome, out);
        self.write_event(&outcome, out);
        self.write_steer(&outcome, out);
        let carry_over = self.settings.carry_over;
        self.covariance.decay(carry_over);
        self.heading.decay(f64::from(carry_over));
        self.gated = 0;
        self.fresh = false;
    }

    fn cal_sigma_deg(&self) -> f32 {
        let solved = self.cal.phase_ready
            && matches!(self.cal.phase, CalPhase::Solved | CalPhase::Warm)
            && self.cal.phase_sigma_deg.is_finite();
        let solve = if solved {
            self.cal.phase_sigma_deg.abs()
        } else {
            DEFAULT_CAL_SIGMA_DEG
        };
        let measured = self.cal.source == Some(CalSourceKind::Emitter)
            || self.manifold.uses_table_at(self.freq_hz());
        let antenna = if measured {
            MEASURED_PHASE_SIGMA_DEG
        } else {
            ANTENNA_PHASE_SIGMA_DEG
        } as f32;
        solve.hypot(antenna)
    }

    fn estimate_now(&mut self) -> Outcome {
        let mut outcome = Outcome {
            heading_deg: self.heading.mean_deg(),
            heading_sigma_deg: self.heading.sigma_deg(),
            ..Outcome::default()
        };
        self.estimate = DoaReport::default();
        self.buffers.spectrum.fill(0);
        if !self.fresh || !self.covariance.matrix(&mut self.matrix) {
            return outcome;
        }
        let snapshots = self.covariance.effective_snapshots() * self.band.independent_fraction();
        let cal_sigma = self.cal_sigma_deg();
        let solved = self.doa.estimate(
            &self.manifold,
            &self.matrix,
            snapshots,
            cal_sigma,
            &mut self.estimate,
        );
        if let Err(error) = solved {
            self.fail();
            outcome.singular = matches!(error, DoaError::Linalg(_));
            return outcome;
        }
        outcome.estimated = true;
        self.fill_spectrum();
        outcome.likelihood = self.fill_likelihood(outcome.heading_deg);
        outcome
    }

    fn fail(&mut self) {
        self.faults.solver_failures += 1;
        self.estimate = DoaReport::default();
        self.covariance.reset();
    }

    fn fill_spectrum(&mut self) {
        let grid = self.doa.grid();
        let azimuths = grid.azimuths();
        let row = self.spectrum_row();
        let spectrum = self.doa.spectrum();
        let Some(values) = spectrum.get(row * azimuths..(row + 1) * azimuths) else {
            return;
        };
        let max = values.iter().copied().fold(0.0f32, f32::max);
        let span = span_db(self.settings.algorithm);
        let step = grid.azimuth_step_deg();
        let start = match grid.spec().span {
            AzimuthSpan::Full => None,
            AzimuthSpan::Half { centre_deg } => Some(centre_deg - 90.0),
        };
        for (degree, byte) in self.buffers.spectrum.iter_mut().enumerate() {
            let index = match start {
                None => ((degree as f64 / step).round() as usize) % azimuths,
                Some(first) => {
                    let offset = norm_deg(degree as f64 - first);
                    (offset / step).round() as usize
                }
            };
            *byte = values
                .get(index)
                .map_or(0, |&value| level(value, max, span));
        }
    }

    fn spectrum_row(&self) -> usize {
        let grid = self.doa.grid();
        let rows = grid.elevations();
        match (grid.spec().elevation, self.estimate.peaks().first()) {
            (Some(span), Some(primary)) if rows > 1 => {
                let step = (span.max_deg - span.min_deg) / (rows - 1) as f64;
                let row = ((primary.elevation_deg - span.min_deg) / step).round();
                (row.max(0.0) as usize).min(rows - 1)
            }
            _ => 0,
        }
    }

    fn fill_likelihood(&mut self, heading_deg: Option<f64>) -> bool {
        let found = self.doa.likelihood(
            &self.manifold,
            &self.matrix,
            &self.estimate,
            &mut self.buffers.likelihood_rel,
        );
        match found {
            Ok(true) => {}
            Ok(false) => return false,
            Err(_) => {
                self.faults.solver_failures += 1;
                return false;
            }
        }
        let buffers = &mut self.buffers;
        match heading_deg {
            Some(heading) => rotate(
                &buffers.likelihood_rel,
                heading,
                &mut buffers.likelihood_out,
            ),
            None => buffers.likelihood_out = buffers.likelihood_rel,
        }
        quantize_likelihood(&buffers.likelihood_out, &mut buffers.likelihood);
        true
    }

    fn df_peak(&self, peak: &Peak, outcome: &Outcome) -> DfPeak {
        DfPeak {
            relative_deg: peak.azimuth_deg as f32,
            true_deg: outcome.true_deg(peak.azimuth_deg).map(|deg| deg as f32),
            elevation_deg: self.settings.elevation.then_some(peak.elevation_deg as f32),
            power_db: decibels(peak.power),
            confidence: peak.confidence,
            sigma_deg: peak.sigma_deg,
            mirror_deg: peak.mirror_deg.map(|deg| deg as f32),
            mirror_true_deg: peak
                .mirror_deg
                .and_then(|deg| outcome.true_deg(deg))
                .map(|deg| deg as f32),
            fit: peak.fit,
        }
    }

    fn write_reading(&mut self, outcome: &Outcome, out: &mut ProcessorOutput<'_>) {
        let Some(slot) = out.report() else {
            return;
        };
        if !matches!(slot, ProcessorReading::Df(_)) {
            *slot = ProcessorReading::Df(DfReading::reserved());
        }
        if let ProcessorReading::Df(reading) = slot {
            self.fill_reading(reading, outcome);
        }
        out.publish_report();
    }

    fn fill_reading(&mut self, reading: &mut DfReading, outcome: &Outcome) {
        stamp_at(&mut reading.at, self.unix_ns);
        reading.peaks.clear();
        for peak in self.estimate.peaks().iter().take(usize::from(MAX_DF_PEAKS)) {
            let entry = self.df_peak(peak, outcome);
            self.faults.push_capped(&mut reading.peaks, entry);
        }
        reading.pseudospectrum.clear();
        extend_capped(
            &mut self.faults,
            &mut reading.pseudospectrum,
            &self.buffers.spectrum,
        );
        reading.likelihood.clear();
        if outcome.likelihood {
            extend_capped(
                &mut self.faults,
                &mut reading.likelihood,
                &self.buffers.likelihood,
            );
        }
        reading.eigenvalues_db.clear();
        let order = self.estimate.order.min(self.estimate.eigenvalues.len());
        for &value in &self.estimate.eigenvalues[..order] {
            self.faults
                .push_capped(&mut reading.eigenvalues_db, decibels(value));
        }
        self.fill_scalars(reading, outcome);
    }

    fn fill_scalars(&self, reading: &mut DfReading, outcome: &Outcome) {
        let estimate = &self.estimate;
        reading.azimuth_deg = outcome.heading_deg;
        reading.station = self.pose.fix.map(|fix| LatLon {
            lat: fix.lat,
            lon: fix.lon,
        });
        reading.sources = u32::try_from(estimate.sources).unwrap_or(u32::MAX);
        reading.sources_auto = self.settings.sources_auto;
        reading.squelched = outcome.estimated && !estimate.squelch_open;
        reading.aliasing = self.alias.aliased;
        reading.algorithm = self.settings.algorithm;
        reading.freq_hz = self.freq_hz();
        reading.heading_sigma_deg = outcome
            .heading_deg
            .map(|_| outcome.heading_sigma_deg as f32);
        reading.likelihood_true = outcome.likelihood && outcome.heading_deg.is_some();
        reading.span_db = span_db(self.settings.algorithm);
        reading.eig_ratio_db = estimate.eig_ratio_db;
        reading.lambda12_db = estimate.lambda12_db;
        reading.snr_db = estimate.snr_db;
        reading.snapshots = estimate.snapshots as f32;
        reading.fit = estimate.fit;
        reading.spacing_ratio = self.alias.spacing_ratio;
        reading.aperture_wavelengths = self.alias.aperture_wavelengths;
        reading.mode_aliasing = self.doa.mode_bias_deg().is_some_and(|bias| bias > 1.0);
        reading.mirror = estimate
            .peaks()
            .iter()
            .any(|peak| peak.mirror_deg.is_some());
        reading.rotating = self.gated > 0;
        reading.singular = outcome.singular;
        reading.table_out_of_range = self.table_out_of_range;
        reading.gated_blocks = self.gated;
    }

    fn write_event(&mut self, outcome: &Outcome, out: &mut ProcessorOutput<'_>) {
        let (Some(primary), Some(heading), Some(fix)) = (
            self.estimate.peaks().first().copied(),
            outcome.heading_deg,
            self.pose.fix,
        ) else {
            return;
        };
        let Some(slot) = out.event() else {
            return;
        };
        if !matches!(slot, DecoderEvent::Df(_)) {
            *slot = DecoderEvent::Df(reserved_bearing(self.node.len()));
        }
        if let DecoderEvent::Df(bearing) = slot {
            self.fill_bearing(bearing, &primary, heading, &fix, outcome);
        }
        out.publish_event();
    }

    fn fill_bearing(
        &mut self,
        bearing: &mut DfBearing,
        primary: &Peak,
        heading: f64,
        fix: &GeoFix,
        outcome: &Outcome,
    ) {
        bearing.bearing_deg = norm_deg(primary.azimuth_deg + heading) as f32;
        bearing.confidence = primary.confidence;
        bearing.lat = Some(fix.lat);
        bearing.lon = Some(fix.lon);
        match &mut bearing.station_id {
            Some(station) => write_text(station, &self.station),
            None => bearing.station_id = Some(self.station.clone()),
        }
        write_text(&mut bearing.node, &self.node);
        bearing.sigma_deg = outcome.event_sigma(primary.sigma_deg);
        bearing.accuracy_m = fix.accuracy_m;
        bearing.heading_deg = Some(heading as f32);
        bearing.heading_sigma_deg = Some(outcome.heading_sigma_deg as f32);
        bearing.relative_deg = Some(primary.azimuth_deg as f32);
        bearing.mirror_deg = primary
            .mirror_deg
            .map(|mirror| norm_deg(mirror + heading) as f32);
        bearing.freq_hz = Some(self.freq_hz());
        bearing.source = BearingSource::Array;
        bearing.moving = self.pose.moving;
        bearing.others.clear();
        for peak in self.estimate.peaks().iter().skip(1).take(MAX_OTHERS) {
            let other = DfOtherPeak {
                bearing_deg: norm_deg(peak.azimuth_deg + heading) as f32,
                sigma_deg: outcome.event_sigma(peak.sigma_deg),
                confidence: peak.confidence,
            };
            self.faults.push_capped(&mut bearing.others, other);
        }
        bearing.likelihood.clear();
        if outcome.likelihood {
            extend_capped(
                &mut self.faults,
                &mut bearing.likelihood,
                &self.buffers.likelihood,
            );
        }
    }

    fn write_steer(&self, outcome: &Outcome, out: &mut ProcessorOutput<'_>) {
        let peaks = self.estimate.peaks();
        let Some(primary) = peaks.first() else {
            return;
        };
        let mut steer = Steer {
            same_array: true,
            relative_deg: primary.azimuth_deg,
            true_deg: outcome.true_deg(primary.azimuth_deg),
            elevation_deg: primary.elevation_deg,
            sigma_deg: primary.sigma_deg,
            wall_ms: self.unix_ns / NANOS_PER_MILLI,
            ..Steer::default()
        };
        for (slot, peak) in peaks.iter().skip(1).take(MAX_OTHERS).enumerate() {
            steer.others_relative_deg[slot] = peak.azimuth_deg;
            steer.others_true_deg[slot] = outcome.true_deg(peak.azimuth_deg);
            steer.others = u8::try_from(slot + 1).unwrap_or(u8::MAX);
        }
        out.steer(steer);
    }
}
