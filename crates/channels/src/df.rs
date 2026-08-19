use num_complex::Complex;
use sdrmm_dsp::{
    Ddc,
    covariance::Covariance,
    music::{Bearing, Music, correlative, peak, quantize},
    steering::{Element, SteeringGrid, ula},
};
use sdrmm_wire::{
    ArrayGeometry, CoherentParams, DF_SPECTRUM_POINTS, DecoderEvent, DfAlgorithm, DfBearing,
    DfParams, DfReading,
};

use crate::{
    ChannelError,
    coherent::{CoherentCtx, CoherentDescriptor, CoherentOutputs, CoherentRx},
};

static DESCRIPTOR: CoherentDescriptor = CoherentDescriptor {
    type_id: "df",
    name: "Direction finder",
    min_lanes: 2,
    needs_phase: true,
};

/// How much of the decimated rate the wanted signal is allowed to fill before the filter starts
/// eating into it.
const PASSBAND_FRACTION: f64 = 0.8;
/// What survives of the covariance from one report to the next: enough to steady a reading,
/// little enough that a bearing that moves is followed rather than smeared.
const CARRY_OVER: f32 = 0.5;
const SPECTRUM_SPAN_DB: f32 = 30.0;

pub struct DfProcessor {
    params: DfParams,
    lanes: usize,
    sample_rate: f64,
    center_hz: f64,
    ddc: Vec<Ddc>,
    baseband: Vec<Vec<Complex<f32>>>,
    covariance: Covariance,
    grid: SteeringGrid,
    music: Music,
    matrix: Vec<Complex<f32>>,
    surface: Vec<f32>,
    scratch: Vec<f32>,
    quantized: Vec<u8>,
    since_report: f64,
    report_samples: f64,
}

#[must_use]
pub fn elements(geometry: &ArrayGeometry) -> Vec<Element> {
    match geometry {
        ArrayGeometry::Uca { radius_m, count } => {
            sdrmm_dsp::steering::uca(*radius_m, *count as usize)
        }
        ArrayGeometry::Ula { spacing_m, count } => ula(*spacing_m, *count as usize),
        ArrayGeometry::Explicit { positions } => positions
            .iter()
            .map(|element| Element::new(element.x_m, element.y_m))
            .collect(),
    }
}

fn params_of(params: &CoherentParams) -> Result<&DfParams, ChannelError> {
    match params {
        CoherentParams::Df(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "direction finder got {} settings",
            other.type_id()
        ))),
    }
}

fn check(params: &DfParams, lanes: usize) -> Result<(), ChannelError> {
    if !params.valid() {
        return Err(ChannelError::InvalidSettings(
            "direction finder settings are outside their allowed ranges".to_owned(),
        ));
    }
    if params.geometry.count() as usize != lanes {
        return Err(ChannelError::InvalidSettings(format!(
            "the array describes {} elements but the radio has {lanes} lanes",
            params.geometry.count()
        )));
    }
    Ok(())
}

impl DfProcessor {
    fn decimated_rate(sample_rate: f64, bandwidth_hz: f64) -> f64 {
        let factor = ((sample_rate * PASSBAND_FRACTION / bandwidth_hz).floor() as usize).max(1);
        sample_rate / factor as f64
    }

    fn build_ddc(&mut self) -> Result<(), ChannelError> {
        let output_rate = Self::decimated_rate(self.sample_rate, self.params.bandwidth_hz);
        self.ddc.clear();
        for _ in 0..self.lanes {
            let ddc = Ddc::new(self.sample_rate, output_rate, self.params.offset_hz)
                .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
            self.ddc.push(ddc);
        }
        self.report_samples = self.sample_rate * f64::from(self.params.report_ms) / 1_000.0;
        Ok(())
    }

    fn rebuild_grid(&mut self) {
        let elements = elements(&self.params.geometry);
        self.grid = SteeringGrid::new(
            &elements,
            self.tuned_hz(),
            360.0 / DF_SPECTRUM_POINTS as f64,
        );
    }

    fn tuned_hz(&self) -> f64 {
        self.center_hz + self.params.offset_hz
    }

    /// The weights that point the array at a bearing: each lane rotated back by the delay the
    /// wavefront would have reached it with, so the lanes add rather than cancel.
    fn weights_for(&self, bearing_deg: f64) -> Vec<Complex<f32>> {
        let aimed = self.params.beam_bearing_deg.unwrap_or(bearing_deg);
        let point = ((aimed.rem_euclid(360.0) / self.grid.step_deg()).round() as usize)
            % self.grid.points().max(1);
        let scale = 1.0 / self.lanes as f32;
        self.grid
            .vector(point)
            .iter()
            .map(|value| value.conj() * scale)
            .collect()
    }

    fn report(&mut self, out: &mut CoherentOutputs) {
        self.covariance.matrix(&mut self.matrix);
        match self.params.algorithm {
            DfAlgorithm::Correlative => correlative(&self.matrix, &self.grid, &mut self.surface),
            DfAlgorithm::Music => self.music.pseudospectrum(
                &self.matrix,
                &self.grid,
                self.params.sources as usize,
                &mut self.surface,
            ),
        }
        let Bearing {
            bearing_deg,
            confidence,
            peak_to_floor_db,
        } = peak(&self.surface, &self.grid, &mut self.scratch);
        self.quantized.resize(DF_SPECTRUM_POINTS, 0);
        quantize(&self.surface, SPECTRUM_SPAN_DB, &mut self.quantized);
        out.bearing = Some(DfReading {
            bearing_deg,
            confidence,
            peak_to_floor_db,
            pseudospectrum: self.quantized.clone(),
        });
        out.events.push(DecoderEvent::Df(DfBearing {
            bearing_deg,
            confidence,
            lat: None,
            lon: None,
            station_id: None,
        }));
        out.weights = Some(self.weights_for(f64::from(bearing_deg)));
        self.covariance.decay(CARRY_OVER);
    }
}

impl CoherentRx for DfProcessor {
    fn descriptor() -> &'static CoherentDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: CoherentCtx, params: &CoherentParams) -> Result<Self, ChannelError> {
        let params = params_of(params)?.clone();
        check(&params, ctx.lanes)?;
        let mut covariance = Covariance::new(ctx.lanes);
        covariance.set_forward_backward(matches!(params.geometry, ArrayGeometry::Ula { .. }));
        let mut processor = Self {
            params,
            lanes: ctx.lanes,
            sample_rate: ctx.sample_rate,
            center_hz: ctx.center_hz,
            ddc: Vec::new(),
            baseband: vec![Vec::new(); ctx.lanes],
            covariance,
            grid: SteeringGrid::new(&[], 1.0, 1.0),
            music: Music::new(ctx.lanes)
                .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?,
            matrix: Vec::new(),
            surface: Vec::new(),
            scratch: Vec::new(),
            quantized: vec![0; DF_SPECTRUM_POINTS],
            since_report: 0.0,
            report_samples: 0.0,
        };
        processor.build_ddc()?;
        processor.rebuild_grid();
        Ok(processor)
    }

    fn apply(&mut self, params: &CoherentParams) -> Result<(), ChannelError> {
        let params = params_of(params)?.clone();
        check(&params, self.lanes)?;
        let rebuild = params.bandwidth_hz != self.params.bandwidth_hz
            || params.offset_hz != self.params.offset_hz
            || params.report_ms != self.params.report_ms;
        let regrid =
            params.geometry != self.params.geometry || params.offset_hz != self.params.offset_hz;
        self.covariance
            .set_forward_backward(matches!(params.geometry, ArrayGeometry::Ula { .. }));
        self.params = params;
        if rebuild {
            self.build_ddc()?;
            self.covariance.reset();
            self.since_report = 0.0;
        }
        if regrid {
            self.rebuild_grid();
        }
        Ok(())
    }

    fn retuned(&mut self, center_hz: f64) {
        self.center_hz = center_hz;
        self.rebuild_grid();
        self.covariance.reset();
        self.since_report = 0.0;
    }

    fn process(&mut self, lanes: &[&[Complex<f32>]], out: &mut CoherentOutputs) {
        let count = lanes.len().min(self.lanes);
        if count < 2 {
            return;
        }
        let mut shortest = usize::MAX;
        for (index, source) in lanes.iter().enumerate().take(count) {
            self.ddc[index].process(source, &mut self.baseband[index]);
            shortest = shortest.min(self.baseband[index].len());
        }
        if shortest == 0 {
            return;
        }
        let view: Vec<&[Complex<f32>]> = self.baseband[..count]
            .iter()
            .map(|lane| &lane[..shortest])
            .collect();
        self.covariance.accumulate(&view);
        self.since_report += lanes[0].len() as f64;
        if self.since_report >= self.report_samples && self.covariance.samples() > 0.0 {
            self.since_report = 0.0;
            self.report(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use sdrmm_wire::{CalParams, DfAlgorithm};

    use super::*;

    const RATE: f64 = 1_024_000.0;
    const CENTRE_HZ: f64 = 300e6;

    fn params(algorithm: DfAlgorithm) -> CoherentParams {
        CoherentParams::Df(DfParams {
            geometry: ArrayGeometry::Uca {
                radius_m: 0.35,
                count: 4,
            },
            algorithm,
            report_ms: 100,
            offset_hz: 25_000.0,
            bandwidth_hz: 20_000.0,
            sources: 1,
            beam_bearing_deg: None,
            cal: CalParams::default(),
        })
    }

    fn wavefront(bearing_deg: f64, len: usize) -> Vec<Vec<Complex<f32>>> {
        let elements = elements(&ArrayGeometry::Uca {
            radius_m: 0.35,
            count: 4,
        });
        let wavelength = sdrmm_dsp::steering::LIGHT_SPEED_M_S / CENTRE_HZ;
        (0..4)
            .map(|lane| {
                let phase = (std::f64::consts::TAU * elements[lane].projected(bearing_deg)
                    / wavelength) as f32;
                let steer = Complex::from_polar(1.0f32, phase);
                (0..len)
                    .map(|k| {
                        let carrier =
                            Complex::from_polar(1.0f32, TAU * 25_000.0 * k as f32 / RATE as f32);
                        carrier * steer
                    })
                    .collect()
            })
            .collect()
    }

    fn read(algorithm: DfAlgorithm, bearing_deg: f64) -> DfReading {
        let ctx = CoherentCtx {
            lanes: 4,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        let mut processor = DfProcessor::new(ctx, &params(algorithm)).expect("builds");
        let mut out = CoherentOutputs::default();
        for _ in 0..8 {
            let block = wavefront(bearing_deg, 32_768);
            let view: Vec<&[Complex<f32>]> = block.iter().map(Vec::as_slice).collect();
            out.reset();
            processor.process(&view, &mut out);
            if let Some(reading) = out.bearing.clone() {
                return reading;
            }
        }
        panic!("no bearing after eight blocks");
    }

    #[test]
    fn a_plane_wave_from_a_known_bearing_reads_back_as_that_bearing() {
        for want in [0.0f64, 137.0, 271.0] {
            let reading = read(DfAlgorithm::Music, want);
            let error = (f64::from(reading.bearing_deg) - want).abs();
            assert!(
                error.min(360.0 - error) < 2.0,
                "wanted {want}, read {}",
                reading.bearing_deg
            );
            assert_eq!(reading.pseudospectrum.len(), DF_SPECTRUM_POINTS);
        }
    }

    #[test]
    fn the_beamformer_finds_the_same_source_the_subspace_estimator_does() {
        let reading = read(DfAlgorithm::Correlative, 137.0);
        let error = (f64::from(reading.bearing_deg) - 137.0).abs();
        assert!(
            error.min(360.0 - error) < 6.0,
            "read {}",
            reading.bearing_deg
        );
    }

    #[test]
    fn a_reading_carries_an_event_with_the_same_bearing() {
        let ctx = CoherentCtx {
            lanes: 4,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        let mut processor = DfProcessor::new(ctx, &params(DfAlgorithm::Music)).expect("builds");
        let mut out = CoherentOutputs::default();
        for _ in 0..8 {
            let block = wavefront(45.0, 32_768);
            let view: Vec<&[Complex<f32>]> = block.iter().map(Vec::as_slice).collect();
            out.reset();
            processor.process(&view, &mut out);
            if let (Some(reading), Some(DecoderEvent::Df(event))) =
                (out.bearing.clone(), out.events.first())
            {
                assert!((reading.bearing_deg - event.bearing_deg).abs() < 1e-6);
                return;
            }
        }
        panic!("no event alongside the reading");
    }

    #[test]
    fn an_array_that_does_not_match_the_radio_is_refused() {
        let ctx = CoherentCtx {
            lanes: 2,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        let Err(ChannelError::InvalidSettings(message)) =
            DfProcessor::new(ctx, &params(DfAlgorithm::Music))
        else {
            panic!("a four-element array on a two-lane radio must be refused");
        };
        assert!(message.contains("lanes"), "{message}");
    }

    #[test]
    fn settings_for_another_processor_are_refused() {
        let ctx = CoherentCtx {
            lanes: 2,
            sample_rate: RATE,
            center_hz: CENTRE_HZ,
        };
        assert!(
            DfProcessor::new(
                ctx,
                &CoherentParams::PassiveRadar(sdrmm_wire::PassiveRadarParams::default())
            )
            .is_err()
        );
    }
}
