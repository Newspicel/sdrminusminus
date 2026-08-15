use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, VorParams, VorReading,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 48_000.0;
const HALF_BANDWIDTH: f64 = 12_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "vor".to_owned(),
    name: "VOR".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("vor".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct VorChannel {
    params: VorParams,
    phase_30: f64,
    phase_subcarrier: f64,
    dc: f64,
    power: f64,
    subcarrier: [Complex<f64>; 4],
    previous_subcarrier: Complex<f64>,
    variable_i: f64,
    variable_q: f64,
    reference_i: f64,
    reference_q: f64,
    subcarrier_level: f64,
    samples: usize,
    settled: bool,
}

fn params(settings: &ChannelSettings) -> Result<&VorParams, ChannelError> {
    match &settings.params {
        ChannelParams::Vor(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "VOR channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(params: &VorParams) -> Result<(), ChannelError> {
    if !(250..=5_000).contains(&params.report_ms) {
        return Err(ChannelError::InvalidSettings(format!(
            "VOR report interval must be 250–5000 ms, got {}",
            params.report_ms
        )));
    }
    if !params.magnetic_declination_deg.is_finite()
        || !(-180.0..=180.0).contains(&params.magnetic_declination_deg)
    {
        return Err(ChannelError::InvalidSettings(format!(
            "VOR magnetic declination must be -180–180 degrees, got {}",
            params.magnetic_declination_deg
        )));
    }
    match (params.station_lat, params.station_lon) {
        (None, None) => Ok(()),
        (Some(lat), Some(lon))
            if lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon) =>
        {
            Ok(())
        }
        _ => Err(ChannelError::InvalidSettings(
            "VOR station latitude and longitude must both be valid".to_owned(),
        )),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-HALF_BANDWIDTH, HALF_BANDWIDTH)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    xng_adapter::channel_filter(RATE, HALF_BANDWIDTH)
}

impl ChannelRx for VorChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?.clone();
        check_params(&params)?;
        Ok(Self::build(params))
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = params(&settings)?.clone();
        check_params(&params)?;
        self.params = params;
        Ok(())
    }

    fn retuned(&mut self) {
        let params = self.params.clone();
        *self = Self::build(params);
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let alpha_dc = 1.0 - (-TAU * 3.0 / RATE).exp();
        let alpha_subcarrier = 1.0 - (-TAU * 1_200.0 / RATE).exp();
        let step_30 = TAU * 30.0 / RATE;
        let step_subcarrier = TAU * 9_960.0 / RATE;
        for sample in iq {
            let envelope = f64::from(sample.norm());
            self.dc += alpha_dc * (envelope - self.dc);
            self.power += alpha_dc * (envelope * envelope - self.power);
            let (sin_30, cos_30) = self.phase_30.sin_cos();
            self.variable_i += envelope * cos_30;
            self.variable_q += envelope * sin_30;
            let (sin_subcarrier, cos_subcarrier) = self.phase_subcarrier.sin_cos();
            let mut filtered = Complex::new(envelope * cos_subcarrier, -envelope * sin_subcarrier);
            for stage in &mut self.subcarrier {
                *stage += (filtered - *stage) * alpha_subcarrier;
                filtered = *stage;
            }
            let discriminator = (filtered * self.previous_subcarrier.conj()).arg() * RATE / TAU;
            self.previous_subcarrier = filtered;
            self.reference_i += discriminator * cos_30;
            self.reference_q += discriminator * sin_30;
            self.subcarrier_level += filtered.norm();
            self.phase_30 = (self.phase_30 + step_30) % TAU;
            self.phase_subcarrier = (self.phase_subcarrier + step_subcarrier) % TAU;
            self.samples += 1;
            if self.samples >= self.report_samples() {
                if self.settled {
                    self.report(out);
                }
                self.settled = true;
                self.clear_window();
            }
        }
    }
}

impl VorChannel {
    fn build(params: VorParams) -> Self {
        Self {
            params,
            phase_30: 0.0,
            phase_subcarrier: 0.0,
            dc: 0.0,
            power: 0.0,
            subcarrier: [Complex::default(); 4],
            previous_subcarrier: Complex::default(),
            variable_i: 0.0,
            variable_q: 0.0,
            reference_i: 0.0,
            reference_q: 0.0,
            subcarrier_level: 0.0,
            samples: 0,
            settled: false,
        }
    }

    fn report_samples(&self) -> usize {
        (RATE * f64::from(self.params.report_ms) / 1_000.0) as usize
    }

    fn report(&self, out: &mut ChannelOutputs) {
        let count = self.samples as f64;
        let variable_amplitude = 2.0 * self.variable_i.hypot(self.variable_q) / count;
        let reference_deviation = 2.0 * self.reference_i.hypot(self.reference_q) / count;
        let modulation = variable_amplitude / self.dc.max(1e-9);
        let subcarrier = self.subcarrier_level / count / self.dc.max(1e-9);
        let confidence = (modulation / 0.3)
            .min(1.0)
            .min((reference_deviation / 480.0).min(1.0))
            .min((subcarrier / 0.15).min(1.0))
            .max(0.0) as f32;
        if confidence < 0.15 {
            return;
        }
        let variable_phase = self.variable_q.atan2(self.variable_i).to_degrees();
        let reference_phase = self.reference_q.atan2(self.reference_i).to_degrees();
        let alpha = 1.0 - (-TAU * 1_200.0 / RATE).exp();
        let omega = TAU * 30.0 / RATE;
        let pole = 1.0 - alpha;
        let filter_lag = 4.0
            * (pole * omega.sin())
                .atan2(1.0 - pole * omega.cos())
                .to_degrees();
        let radial = (variable_phase - reference_phase + filter_lag).rem_euclid(360.0);
        out.events.push(DecoderEvent::Vor(VorReading {
            station: self.params.station.clone(),
            station_lat: self.params.station_lat,
            station_lon: self.params.station_lon,
            magnetic_declination_deg: self.params.magnetic_declination_deg,
            radial_deg: radial,
            variable_phase_deg: variable_phase.rem_euclid(360.0),
            reference_phase_deg: reference_phase.rem_euclid(360.0),
            signal_db: (10.0 * self.power.max(1e-12).log10()) as f32,
            confidence,
        }));
    }

    fn clear_window(&mut self) {
        self.variable_i = 0.0;
        self.variable_q = 0.0;
        self.reference_i = 0.0;
        self.reference_q = 0.0;
        self.subcarrier_level = 0.0;
        self.samples = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn measures_the_radial_from_an_analytic_vor_signal() {
        for radial in [0.0, 90.0, 123.0, 270.0] {
            let params = VorParams {
                station: Some("TST".to_owned()),
                ..VorParams::default()
            };
            let mut channel = VorChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Vor(params)),
            )
            .expect("channel");
            let events = run_events(&mut channel, &crate::testgen::vor::transmission(radial, 2));
            let reading = events
                .iter()
                .filter_map(|event| match event {
                    DecoderEvent::Vor(reading) => Some(reading),
                    _ => None,
                })
                .next_back()
                .expect("VOR reading");
            let error = (reading.radial_deg - radial + 180.0).rem_euclid(360.0) - 180.0;
            assert!(error.abs() < 0.5, "{radial} => {}", reading.radial_deg);
            assert!(reading.confidence > 0.8);
        }
    }

    #[test]
    fn processing_keeps_ahead_of_the_channel_rate() {
        let iq = crate::testgen::vor::transmission(45.0, 5);
        let mut channel = VorChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Vor(VorParams::default())),
        )
        .expect("channel");
        let started = Instant::now();
        let events = run_events(&mut channel, &iq);
        assert!(!events.is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
