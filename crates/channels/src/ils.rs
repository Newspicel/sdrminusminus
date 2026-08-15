use std::{f64::consts::TAU, sync::LazyLock};

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, IlsComponent, IlsParams,
    IlsReading,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 48_000.0;
const HALF_BANDWIDTH: f64 = 10_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "ils".to_owned(),
    name: "ILS localizer / glideslope".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("ils".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct IlsChannel {
    params: IlsParams,
    phase_90: f64,
    phase_150: f64,
    dc: f64,
    power: f64,
    tone_90_i: f64,
    tone_90_q: f64,
    tone_150_i: f64,
    tone_150_q: f64,
    envelope_sum: f64,
    samples: usize,
}

fn params(settings: &ChannelSettings) -> Result<&IlsParams, ChannelError> {
    match &settings.params {
        ChannelParams::Ils(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "ILS channel got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(params: &IlsParams) -> Result<(), ChannelError> {
    if (250..=5_000).contains(&params.report_ms) {
        Ok(())
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "ILS report interval must be 250–5000 ms, got {}",
            params.report_ms
        )))
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-HALF_BANDWIDTH, HALF_BANDWIDTH)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    xng_adapter::channel_filter(RATE, HALF_BANDWIDTH)
}

impl ChannelRx for IlsChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = *params(&settings)?;
        check_params(&params)?;
        Ok(Self::build(params))
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = *params(&settings)?;
        check_params(&params)?;
        self.params = params;
        Ok(())
    }

    fn retuned(&mut self) {
        *self = Self::build(self.params);
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let alpha = 1.0 - (-TAU * 3.0 / RATE).exp();
        let step_90 = TAU * 90.0 / RATE;
        let step_150 = TAU * 150.0 / RATE;
        for sample in iq {
            let envelope = f64::from(sample.norm());
            self.dc += alpha * (envelope - self.dc);
            self.power += alpha * (envelope * envelope - self.power);
            let ac = envelope - self.dc;
            let (sin_90, cos_90) = self.phase_90.sin_cos();
            let (sin_150, cos_150) = self.phase_150.sin_cos();
            self.tone_90_i += ac * cos_90;
            self.tone_90_q += ac * sin_90;
            self.tone_150_i += ac * cos_150;
            self.tone_150_q += ac * sin_150;
            self.envelope_sum += envelope;
            self.phase_90 = (self.phase_90 + step_90) % TAU;
            self.phase_150 = (self.phase_150 + step_150) % TAU;
            self.samples += 1;
            if self.samples >= self.report_samples() {
                self.report(out);
                self.clear_window();
            }
        }
    }
}

impl IlsChannel {
    fn build(params: IlsParams) -> Self {
        Self {
            params,
            phase_90: 0.0,
            phase_150: 0.0,
            dc: 0.0,
            power: 0.0,
            tone_90_i: 0.0,
            tone_90_q: 0.0,
            tone_150_i: 0.0,
            tone_150_q: 0.0,
            envelope_sum: 0.0,
            samples: 0,
        }
    }

    fn report_samples(&self) -> usize {
        (RATE * f64::from(self.params.report_ms) / 1_000.0) as usize
    }

    fn report(&self, out: &mut ChannelOutputs) {
        let count = self.samples as f64;
        let mean = self.envelope_sum / count;
        let modulation_90 = (2.0 * self.tone_90_i.hypot(self.tone_90_q) / count / mean) as f32;
        let modulation_150 = (2.0 * self.tone_150_i.hypot(self.tone_150_q) / count / mean) as f32;
        if modulation_90 + modulation_150 < 0.08 {
            return;
        }
        let ddm = modulation_90 - modulation_150;
        let deviation_dots = match self.params.component {
            IlsComponent::Localizer => ddm / 0.155 * 2.5,
            IlsComponent::Glideslope => ddm / 0.175 * 2.0,
        };
        out.events.push(DecoderEvent::Ils(IlsReading {
            component: self.params.component,
            modulation_90,
            modulation_150,
            ddm,
            deviation_dots,
            signal_db: (10.0 * self.power.max(1e-12).log10()) as f32,
        }));
    }

    fn clear_window(&mut self) {
        self.tone_90_i = 0.0;
        self.tone_90_q = 0.0;
        self.tone_150_i = 0.0;
        self.tone_150_q = 0.0;
        self.envelope_sum = 0.0;
        self.samples = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn measures_ils_difference_in_depth_of_modulation() {
        let iq: Vec<_> = (0..RATE as usize)
            .map(|index| {
                let time = index as f64 / RATE;
                let envelope =
                    1.0 + 0.28 * (TAU * 90.0 * time).cos() + 0.12 * (TAU * 150.0 * time).cos();
                Complex::new(envelope as f32, 0.0)
            })
            .collect();
        let mut channel = IlsChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Ils(IlsParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        let reading = events
            .iter()
            .filter_map(|event| match event {
                DecoderEvent::Ils(reading) => Some(reading),
                _ => None,
            })
            .next_back()
            .expect("ILS reading");
        assert!((reading.ddm - 0.16).abs() < 0.01, "{}", reading.ddm);
        assert!((reading.deviation_dots - 2.58).abs() < 0.2);
    }

    #[test]
    fn processing_keeps_ahead_of_the_channel_rate() {
        let iq: Vec<_> = (0..RATE as usize * 5)
            .map(|index| {
                let time = index as f64 / RATE;
                let envelope =
                    1.0 + 0.2 * (TAU * 90.0 * time).cos() + 0.2 * (TAU * 150.0 * time).cos();
                Complex::new(envelope as f32, 0.0)
            })
            .collect();
        let mut channel = IlsChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Ils(IlsParams::default())),
        )
        .expect("channel");
        let started = Instant::now();
        let events = run_events(&mut channel, &iq);
        assert!(!events.is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
