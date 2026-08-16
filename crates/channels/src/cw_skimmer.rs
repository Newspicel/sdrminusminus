use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Ddc, Decimator, SpectrumAnalyzer, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, CwSkimmerParams, CwSkimmerSpot,
    DecoderEvent, MorseParams,
};

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, MorseChannel,
    check_input_rate,
};

const RATE: f64 = 48_000.0;
const MORSE_RATE: f64 = 8_000.0;
const FFT_SIZE: usize = 4_096;
const FFT_HOP: usize = 2_048;
const TRACK_SEPARATION_HZ: f32 = 180.0;
const MATCH_HZ: f32 = 90.0;
const TRACK_MISSES: u16 = 150;
const CHANNEL_TAPS: usize = 257;
const TRACK_TAPS: usize = 129;
const MAX_SIGNALS: u16 = 128;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "cw_skimmer".to_owned(),
    name: "CW skimmer".to_owned(),
    bandwidth_hz: 24_000.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("cw_skimmer".to_owned()),
    ..ChannelDescriptor::default()
});

struct Track {
    frequency_hz: f32,
    snr_db: f32,
    misses: u16,
    ddc: Ddc,
    filter: Decimator,
    morse: MorseChannel,
    mixed: Vec<Complex<f32>>,
    narrow: Vec<Complex<f32>>,
    output: ChannelOutputs,
}

impl Track {
    fn prototype(wpm: Option<f32>) -> Result<Self, ChannelError> {
        let morse_settings = ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Morse(MorseParams {
                bandwidth_hz: 400.0,
                wpm,
            }),
            audio: Default::default(),
        };
        Ok(Self {
            frequency_hz: 0.0,
            snr_db: 0.0,
            misses: 0,
            ddc: Ddc::new(RATE, MORSE_RATE, 0.0)
                .map_err(|error| ChannelError::InvalidSettings(error.to_string()))?,
            filter: Decimator::new(&design_lowpass(TRACK_TAPS, 250.0 / MORSE_RATE), 1),
            morse: MorseChannel::new(
                ChannelCtx {
                    input_rate: MORSE_RATE,
                },
                morse_settings,
            )?,
            mixed: Vec::new(),
            narrow: Vec::new(),
            output: ChannelOutputs::default(),
        })
    }

    fn spawn(&self, frequency_hz: f32, snr_db: f32) -> Self {
        let mut ddc = self.ddc.clone();
        ddc.set_offset(f64::from(frequency_hz));
        Self {
            frequency_hz,
            snr_db,
            misses: 0,
            ddc,
            filter: self.filter.clone(),
            morse: self.morse.clone(),
            mixed: Vec::new(),
            narrow: Vec::new(),
            output: ChannelOutputs::default(),
        }
    }

    fn tune(&mut self, frequency_hz: f32, snr_db: f32) {
        self.frequency_hz = self.frequency_hz * 0.8 + frequency_hz * 0.2;
        self.snr_db = self.snr_db * 0.8 + snr_db * 0.2;
        self.ddc.set_offset(f64::from(self.frequency_hz));
        self.misses = 0;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.ddc.process(iq, &mut self.mixed);
        self.filter.process(&self.mixed, &mut self.narrow);
        self.output.reset();
        self.morse.process(&self.narrow, &mut self.output);
        for event in self.output.events.drain(..) {
            let DecoderEvent::Morse(message) = event else {
                continue;
            };
            out.events.push(DecoderEvent::CwSkimmer(CwSkimmerSpot {
                offset_hz: self.frequency_hz,
                text: message.text,
                wpm: message.wpm,
                snr_db: self.snr_db,
            }));
        }
    }
}

pub struct CwSkimmerChannel {
    bandwidth_hz: f64,
    threshold_db: f32,
    max_signals: u16,
    prototype: Track,
    analyzer: SpectrumAnalyzer,
    samples: Vec<Complex<f32>>,
    power: Vec<f32>,
    floor: Vec<f32>,
    candidates: Vec<(f32, f32)>,
    tracks: Vec<Track>,
}

fn params(settings: &ChannelSettings) -> Result<&CwSkimmerParams, ChannelError> {
    match &settings.params {
        ChannelParams::CwSkimmer(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "cw skimmer got {} params",
            other.type_id()
        ))),
    }
}

fn check_params(params: &CwSkimmerParams) -> Result<(), ChannelError> {
    let widest = DESCRIPTOR.bandwidth_hz;
    if !(params.bandwidth_hz.is_finite() && (1_000.0..=widest).contains(&params.bandwidth_hz)) {
        return Err(ChannelError::InvalidSettings(format!(
            "cw skimmer bandwidth must be in [1000, {widest}] Hz, got {}",
            params.bandwidth_hz
        )));
    }
    if !(params.threshold_db.is_finite() && (3.0..=40.0).contains(&params.threshold_db)) {
        return Err(ChannelError::InvalidSettings(format!(
            "cw skimmer threshold must be in [3, 40] dB, got {}",
            params.threshold_db
        )));
    }
    if !(1..=MAX_SIGNALS).contains(&params.max_signals) {
        return Err(ChannelError::InvalidSettings(format!(
            "cw skimmer max signals must be in [1, {MAX_SIGNALS}], got {}",
            params.max_signals
        )));
    }
    if let Some(wpm) = params.wpm
        && !(wpm.is_finite() && (3.0..=80.0).contains(&wpm))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "cw skimmer wpm must be in [3, 80], got {wpm}"
        )));
    }
    Ok(())
}

pub(crate) fn occupied_band(params: &CwSkimmerParams) -> (f64, f64) {
    let half = params.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(params: &CwSkimmerParams) -> Result<ChannelFilter, ChannelError> {
    check_params(params)?;
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, params.bandwidth_hz / 2.0 / RATE),
        1,
    )))
}

impl CwSkimmerChannel {
    fn configure(&mut self, params: &CwSkimmerParams) -> Result<(), ChannelError> {
        self.bandwidth_hz = params.bandwidth_hz;
        self.threshold_db = params.threshold_db;
        self.max_signals = params.max_signals;
        self.prototype = Track::prototype(params.wpm)?;
        self.tracks.truncate(usize::from(self.max_signals));
        Ok(())
    }

    fn analyze(&mut self) {
        self.analyzer
            .power_db(&self.samples[..FFT_SIZE], &mut self.power);
        self.floor.clear();
        self.floor
            .extend(self.power.iter().copied().filter(|value| value.is_finite()));
        let floor = if self.floor.is_empty() {
            return;
        } else {
            let at = self.floor.len() / 4;
            let (_, value, _) = self.floor.select_nth_unstable_by(at, f32::total_cmp);
            *value
        };
        self.candidates.clear();
        let bin_hz = RATE as f32 / FFT_SIZE as f32;
        let half_band = self.bandwidth_hz as f32 / 2.0;
        for index in 2..FFT_SIZE - 2 {
            let frequency = (index as f32 - FFT_SIZE as f32 / 2.0) * bin_hz;
            let power = self.power[index];
            if frequency.abs() <= half_band
                && power >= floor + self.threshold_db
                && power > self.power[index - 1]
                && power >= self.power[index + 1]
                && power - self.power[index - 2].max(self.power[index + 2]) >= 3.0
            {
                self.candidates.push((frequency, power - floor));
            }
        }
        self.candidates
            .sort_by(|left, right| right.1.total_cmp(&left.1));
        for track in &mut self.tracks {
            track.misses = track.misses.saturating_add(1);
        }
        for &(frequency, snr) in &self.candidates {
            if let Some(track) = self
                .tracks
                .iter_mut()
                .min_by(|left, right| {
                    (left.frequency_hz - frequency)
                        .abs()
                        .total_cmp(&(right.frequency_hz - frequency).abs())
                })
                .filter(|track| (track.frequency_hz - frequency).abs() <= MATCH_HZ)
            {
                track.tune(frequency, snr);
                continue;
            }
            if self.tracks.len() >= usize::from(self.max_signals)
                || self
                    .tracks
                    .iter()
                    .any(|track| (track.frequency_hz - frequency).abs() < TRACK_SEPARATION_HZ)
            {
                continue;
            }
            self.tracks.push(self.prototype.spawn(frequency, snr));
        }
        self.tracks.retain(|track| track.misses < TRACK_MISSES);
    }
}

impl ChannelRx for CwSkimmerChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        check_params(params)?;
        Ok(Self {
            bandwidth_hz: params.bandwidth_hz,
            threshold_db: params.threshold_db,
            max_signals: params.max_signals,
            prototype: Track::prototype(params.wpm)?,
            analyzer: SpectrumAnalyzer::new(FFT_SIZE),
            samples: Vec::with_capacity(FFT_SIZE + FFT_HOP),
            power: vec![0.0; FFT_SIZE],
            floor: Vec::with_capacity(FFT_SIZE),
            candidates: Vec::with_capacity(usize::from(params.max_signals) * 4),
            tracks: Vec::with_capacity(usize::from(params.max_signals)),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let params = params(&settings)?;
        check_params(params)?;
        self.configure(params)
    }

    fn retuned(&mut self) {
        self.samples.clear();
        self.tracks.clear();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for track in &mut self.tracks {
            track.process(iq, out);
        }
        self.samples.extend_from_slice(iq);
        while self.samples.len() >= FFT_SIZE {
            self.analyze();
            self.samples.drain(..FFT_HOP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testgen, testutil::settings};

    fn channel(params: CwSkimmerParams) -> Result<CwSkimmerChannel, ChannelError> {
        CwSkimmerChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::CwSkimmer(params)),
        )
    }

    #[test]
    fn the_passband_never_reaches_past_what_the_descriptor_advertises() {
        let widest = CwSkimmerParams {
            bandwidth_hz: DESCRIPTOR.bandwidth_hz,
            ..CwSkimmerParams::default()
        };
        let (low, high) = occupied_band(&widest);
        assert!(high - low <= DESCRIPTOR.bandwidth_hz);
        assert!(channel(widest).is_ok());
        assert!(
            channel(CwSkimmerParams {
                bandwidth_hz: DESCRIPTOR.bandwidth_hz + 1.0,
                ..CwSkimmerParams::default()
            })
            .is_err()
        );
    }

    #[test]
    fn decodes_two_cw_signals_in_the_same_passband() {
        let first = testgen::morse::transmission("VVV VVV CQ DE DL1AAA K", 18.0, -3_500.0, RATE);
        let second = testgen::morse::transmission("VVV VVV CQ DE G4BBB K", 27.0, 4_200.0, RATE);
        let length = first.len().max(second.len()) + RATE as usize * 4;
        let mut iq = vec![Complex::new(0.0, 0.0); length];
        for (destination, source) in iq.iter_mut().zip(first) {
            *destination += source * 0.55;
        }
        for (destination, source) in iq.iter_mut().zip(second) {
            *destination += source * 0.35;
        }
        testgen::add_noise(&mut iq, 17, 0.002);
        let mut channel = CwSkimmerChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::CwSkimmer(CwSkimmerParams {
                bandwidth_hz: 16_000.0,
                threshold_db: 8.0,
                max_signals: 8,
                wpm: None,
            })),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        for chunk in iq.chunks(2_047) {
            channel.process(chunk, &mut out);
        }
        let spots = out
            .events
            .iter()
            .filter_map(|event| match event {
                DecoderEvent::CwSkimmer(spot) => Some(spot),
                _ => None,
            })
            .collect::<Vec<_>>();
        let first_text = spots
            .iter()
            .filter(|spot| spot.offset_hz < 0.0)
            .map(|spot| spot.text.as_str())
            .collect::<String>();
        let second_text = spots
            .iter()
            .filter(|spot| spot.offset_hz > 0.0)
            .map(|spot| spot.text.as_str())
            .collect::<String>();
        assert!(first_text.contains("DL1AAA"), "{first_text:?}");
        assert!(second_text.contains("G4BBB"), "{second_text:?}");
    }
}
