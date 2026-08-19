use num_complex::Complex;
use sdrmm_dsp::{
    caf::{Caf, Surface},
    cfar::{self, CfarParams},
    eca::{Eca, EcaParams},
    track::{Tracker, TrackerParams},
};
use sdrmm_wire::{CoherentParams, DecoderEvent, PassiveRadarParams, RadarDetection};

use crate::{
    ChannelError,
    coherent::{CoherentCtx, CoherentDescriptor, CoherentOutputs, CoherentRx, RangeDopplerSurface},
};

static DESCRIPTOR: CoherentDescriptor = CoherentDescriptor {
    type_id: "passive_radar",
    name: "Passive radar",
    min_lanes: 2,
    needs_phase: false,
};

/// The reference antenna is lane zero and the surveillance antenna lane one, which is the same
/// order the node's `ref` and `surv` ports are wired in.
const REFERENCE: usize = 0;
const SURVEILLANCE: usize = 1;

const SURFACE_SPAN_DB: f32 = 40.0;
const LIGHT_SPEED_KM_S: f32 = 299_792.5;

pub struct PassiveRadarProcessor {
    params: PassiveRadarParams,
    sample_rate: f64,
    cpi: usize,
    eca: Eca,
    caf: Caf,
    reference: Vec<Complex<f32>>,
    surveillance: Vec<Complex<f32>>,
    residual: Vec<Complex<f32>>,
    surface: Surface,
    detections: Vec<cfar::Detection>,
    cells: Vec<u8>,
    tracker: Tracker,
    looks: Vec<(f32, f32)>,
    named: Vec<Option<u32>>,
}

fn params_of(params: &CoherentParams) -> Result<&PassiveRadarParams, ChannelError> {
    match params {
        CoherentParams::PassiveRadar(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "passive radar got {} settings",
            other.type_id()
        ))),
    }
}

fn cpi_samples(sample_rate: f64, cpi_ms: u32) -> usize {
    ((sample_rate * f64::from(cpi_ms) / 1_000.0).round() as usize).max(64)
}

fn dopplers(cpi: usize, sample_rate: f64, span_hz: f64) -> usize {
    let step = sample_rate / cpi as f64;
    let half = (span_hz / 2.0 / step).round() as usize;
    (2 * half + 1).max(1)
}

fn eca_of(params: &PassiveRadarParams) -> EcaParams {
    EcaParams {
        delay_taps: params.eca.delay_taps as usize,
        doppler_bins: params.eca.doppler_bins as usize,
        batch: params.eca.batch_samples as usize,
        loading: params.eca.loading,
    }
}

fn cfar_of(params: &PassiveRadarParams) -> CfarParams {
    CfarParams {
        guard_range: params.cfar.guard_range as usize,
        guard_doppler: params.cfar.guard_doppler as usize,
        train_range: params.cfar.train_range as usize,
        train_doppler: params.cfar.train_doppler as usize,
        probability_false_alarm: params.cfar.probability_false_alarm,
        min_snr_db: params.cfar.min_snr_db,
        zero_doppler_guard: params.cfar.zero_doppler_guard as usize,
    }
}

impl PassiveRadarProcessor {
    fn build(
        params: &PassiveRadarParams,
        sample_rate: f64,
    ) -> Result<(Eca, Caf, usize), ChannelError> {
        if !params.valid() {
            return Err(ChannelError::InvalidSettings(
                "passive radar settings are outside their allowed ranges".to_owned(),
            ));
        }
        let cpi = cpi_samples(sample_rate, params.cpi_ms);
        let eca = Eca::new(eca_of(params), sample_rate)
            .map_err(|e| ChannelError::InvalidSettings(e.to_string()))?;
        let ranges = (params.max_range_bins as usize).min(cpi);
        let caf = Caf::new(
            cpi,
            ranges,
            dopplers(cpi, sample_rate, params.doppler_span_hz),
            sample_rate,
        );
        Ok((eca, caf, cpi))
    }

    /// Runs one coherent processing interval once both lanes have handed over a full one.
    fn integrate(&mut self, out: &mut CoherentOutputs) {
        self.eca.cancel(
            &self.reference[..self.cpi],
            &self.surveillance[..self.cpi],
            &mut self.residual,
        );
        self.caf.compute(
            &self.reference[..self.cpi],
            &self.residual,
            &mut self.surface,
        );
        cfar::detect(
            &self.surface.power,
            self.surface.ranges,
            self.surface.dopplers,
            &cfar_of(&self.params),
            &mut self.detections,
        );
        let peak = self
            .surface
            .power
            .iter()
            .copied()
            .fold(f32::MIN_POSITIVE, f32::max);
        let top = 10.0 * peak.max(1e-30).log10();
        self.cells.clear();
        self.cells.reserve(self.surface.power.len());
        for value in &self.surface.power {
            let db = 10.0 * value.max(1e-30).log10();
            let level = ((db - top + SURFACE_SPAN_DB) / SURFACE_SPAN_DB * 255.0).clamp(0.0, 255.0);
            self.cells.push(level as u8);
        }
        out.surface = Some(RangeDopplerSurface {
            ranges: self.surface.ranges,
            dopplers: self.surface.dopplers,
            range_step_s: self.surface.range_step_s,
            doppler_step_hz: self.surface.doppler_step_hz,
            db_min: top - SURFACE_SPAN_DB,
            db_max: top,
            cells: std::mem::take(&mut self.cells),
        });
        cfar::cluster(&mut self.detections);
        self.looks.clear();
        self.looks.extend(self.detections.iter().map(|hit| {
            (
                hit.range_bin as f32,
                self.surface.doppler_hz(hit.doppler_bin),
            )
        }));
        self.tracker.update(&self.looks, &mut self.named);
        for (hit, track_id) in self.detections.iter().zip(&self.named) {
            let detection = RadarDetection {
                range_bin: hit.range_bin as u32,
                range_km: hit.range_bin as f32 * self.surface.range_step_s * LIGHT_SPEED_KM_S,
                doppler_hz: self.surface.doppler_hz(hit.doppler_bin),
                snr_db: hit.snr_db,
                track_id: *track_id,
            };
            out.detections.push(detection);
            out.events.push(DecoderEvent::Radar(detection));
        }
        self.reference.drain(..self.cpi);
        self.surveillance.drain(..self.cpi);
    }
}

impl CoherentRx for PassiveRadarProcessor {
    fn descriptor() -> &'static CoherentDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: CoherentCtx, params: &CoherentParams) -> Result<Self, ChannelError> {
        let params = *params_of(params)?;
        let (eca, caf, cpi) = Self::build(&params, ctx.sample_rate)?;
        Ok(Self {
            params,
            sample_rate: ctx.sample_rate,
            cpi,
            eca,
            caf,
            reference: Vec::with_capacity(cpi * 2),
            surveillance: Vec::with_capacity(cpi * 2),
            residual: Vec::new(),
            surface: Surface::default(),
            detections: Vec::new(),
            cells: Vec::new(),
            tracker: Tracker::new(TrackerParams::default()),
            looks: Vec::new(),
            named: Vec::new(),
        })
    }

    fn apply(&mut self, params: &CoherentParams) -> Result<(), ChannelError> {
        let params = *params_of(params)?;
        if params == self.params {
            return Ok(());
        }
        let (eca, caf, cpi) = Self::build(&params, self.sample_rate)?;
        self.params = params;
        self.eca = eca;
        self.caf = caf;
        self.cpi = cpi;
        self.reference.clear();
        self.surveillance.clear();
        Ok(())
    }

    fn process(&mut self, lanes: &[&[Complex<f32>]], out: &mut CoherentOutputs) {
        let (Some(reference), Some(surveillance)) = (lanes.get(REFERENCE), lanes.get(SURVEILLANCE))
        else {
            return;
        };
        self.reference.extend_from_slice(reference);
        self.surveillance.extend_from_slice(surveillance);
        while self.reference.len() >= self.cpi && self.surveillance.len() >= self.cpi {
            self.integrate(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    const RATE: f64 = 1_000_000.0;

    fn params() -> CoherentParams {
        CoherentParams::PassiveRadar(PassiveRadarParams {
            cpi_ms: 10,
            max_range_bins: 128,
            doppler_span_hz: 2_000.0,
            eca: sdrmm_wire::EcaParams {
                delay_taps: 8,
                doppler_bins: 0,
                batch_samples: 10_000,
                loading: 1e-6,
            },
            cfar: sdrmm_wire::CfarParams::default(),
            illuminator: None,
        })
    }

    fn noise(len: usize, seed: u64) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                let mut next = || {
                    state ^= state >> 12;
                    state ^= state << 25;
                    state ^= state >> 27;
                    (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32
                        - 1.0
                };
                Complex::new(next(), next())
            })
            .collect()
    }

    fn run(delay: usize, doppler_hz: f32, echo_gain: f32) -> CoherentOutputs {
        looks(delay, doppler_hz, echo_gain, 1)
            .pop()
            .expect("one look")
    }

    /// Several integrations of the same scene, which is what the tracker needs before it will
    /// call anything a target.
    fn looks(delay: usize, doppler_hz: f32, echo_gain: f32, rounds: usize) -> Vec<CoherentOutputs> {
        let ctx = CoherentCtx {
            lanes: 2,
            sample_rate: RATE,
            center_hz: 100e6,
        };
        let mut processor = PassiveRadarProcessor::new(ctx, &params()).expect("builds");
        let len = 10_000;
        let reference = noise(len, 0x5AD4);
        let mut surveillance: Vec<Complex<f32>> = reference
            .iter()
            .map(|value| value * Complex::from_polar(8.0f32, 0.3))
            .collect();
        if echo_gain > 0.0 {
            for index in delay..len {
                let phase = TAU * doppler_hz * index as f32 / RATE as f32;
                surveillance[index] +=
                    reference[index - delay] * Complex::from_polar(echo_gain, phase);
            }
        }
        let mut all = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let mut out = CoherentOutputs::default();
            processor.process(&[&reference, &surveillance], &mut out);
            all.push(out);
        }
        all
    }

    #[test]
    fn an_echo_shows_up_at_the_range_and_doppler_it_was_built_with() {
        let out = run(40, 200.0, 0.05);
        let surface = out.surface.as_ref().expect("a surface every interval");
        assert_eq!(surface.ranges, 128);
        assert!(surface.dopplers >= 3);
        assert!(
            out.detections
                .iter()
                .any(|d| d.range_bin == 40 && (d.doppler_hz - 200.0).abs() < 60.0),
            "{:?}",
            out.detections
        );
    }

    #[test]
    fn a_direct_path_with_nothing_behind_it_produces_no_detections() {
        let out = run(0, 0.0, 0.0);
        assert!(out.surface.is_some());
        assert!(out.detections.is_empty(), "{:?}", out.detections);
    }

    #[test]
    fn an_echo_that_keeps_coming_back_is_given_a_name() {
        let rounds = looks(40, 200.0, 0.05, 4);
        let first = rounds.first().expect("a first look");
        assert!(
            first.detections.iter().all(|d| d.track_id.is_none()),
            "one look decides nothing: {:?}",
            first.detections
        );
        let echo = |out: &CoherentOutputs| {
            out.detections
                .iter()
                .find(|d| d.range_bin == 40 && (d.doppler_hz - 200.0).abs() < 60.0)
                .and_then(|d| d.track_id)
        };
        let named: Vec<Option<u32>> = rounds.iter().map(echo).collect();
        let last = named.last().copied().flatten();
        assert!(
            last.is_some(),
            "an echo that is there every time is a target: {named:?}"
        );
        assert!(
            named[2..].iter().all(|id| *id == last),
            "the same echo keeps the same name: {named:?}"
        );
    }

    #[test]
    fn every_detection_is_repeated_as_an_event() {
        let out = run(40, 200.0, 0.05);
        assert_eq!(out.detections.len(), out.events.len());
        for (detection, event) in out.detections.iter().zip(&out.events) {
            match event {
                DecoderEvent::Radar(reported) => assert_eq!(reported, detection),
                other => panic!("a radar detection came out as {other:?}"),
            }
        }
    }

    #[test]
    fn settings_for_another_processor_are_refused() {
        let ctx = CoherentCtx {
            lanes: 2,
            sample_rate: RATE,
            center_hz: 100e6,
        };
        assert!(
            PassiveRadarProcessor::new(ctx, &CoherentParams::Df(sdrmm_wire::DfParams::default()))
                .is_err()
        );
    }
}
