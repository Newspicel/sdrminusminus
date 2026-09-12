use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Ddc, Decimator, NoiseFloor, SpectrumAnalyzer, design_lowpass};
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
const FLOOR_HALF_BINS: usize = 48;
const FLOOR_STRIDE_BINS: usize = 8;
const INIT_HITS: u8 = 6;
const INIT_FRAMES: u8 = 24;
const PROVE_CHUNKS: u8 = 3;
const PROVE_FRAMES: u16 = 240;
/// Timing this far off the one-dot and three-dot lengths came from a slicer chewing on noise, not
/// from a hand or a keyer.
const FIT_MAX: f32 = 0.35;
const ECHO_MARGIN_DB: f32 = 6.0;
/// No receiver hears a station this far under the loudest one in its passband; a peak that deep is
/// the analysis window's own skirt, not another operator.
const SPUR_RANGE_DB: f32 = 80.0;

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
    age: u16,
    chunks: u8,
    proven: bool,
    held: Vec<CwSkimmerSpot>,
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
            frequency_hz: 0.0,
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
            age: 0,
            chunks: 0,
            proven: false,
            held: Vec::new(),
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
            age: 0,
            chunks: 0,
            proven: false,
            held: Vec::with_capacity(usize::from(PROVE_CHUNKS)),
            ddc,
            filter: self.filter.clone(),
            morse: self.morse.clone(),
            mixed: Vec::new(),
            narrow: Vec::new(),
            output: ChannelOutputs::default(),
        }
    }

    fn spent(&self) -> bool {
        !self.proven && (self.chunks >= PROVE_CHUNKS || self.age >= PROVE_FRAMES)
    }

    fn tune(&mut self, frequency_hz: f32, snr_db: f32) {
        self.frequency_hz = self.frequency_hz * 0.8 + frequency_hz * 0.2;
        self.snr_db = self.snr_db * 0.8 + snr_db * 0.2;
        self.ddc.set_offset(f64::from(self.frequency_hz));
        self.misses = 0;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<CwSkimmerSpot>) {
        self.ddc.process(iq, &mut self.mixed);
        self.filter.process(&self.mixed, &mut self.narrow);
        self.output.reset();
        self.morse.process(&self.narrow, &mut self.output);
        for event in self.output.events.drain(..) {
            let DecoderEvent::Morse(message) = event else {
                continue;
            };
            let spot = CwSkimmerSpot {
                offset_hz: self.frequency_hz,
                text: message.text,
                wpm: message.wpm,
                snr_db: self.snr_db,
            };
            if self.proven {
                out.push(spot);
                continue;
            }
            self.chunks = self.chunks.saturating_add(1);
            if self.morse.element_fit() <= FIT_MAX {
                self.proven = true;
                out.append(&mut self.held);
                out.push(spot);
            } else if self.chunks >= PROVE_CHUNKS {
                self.held.clear();
            } else {
                self.held.push(spot);
            }
        }
    }
}

struct Pending {
    frequency_hz: f32,
    snr_db: f32,
    hits: u8,
    age: u8,
}

pub struct CwSkimmerChannel {
    bandwidth_hz: f64,
    threshold_db: f32,
    max_signals: u16,
    prototype: Track,
    analyzer: SpectrumAnalyzer,
    noise: NoiseFloor,
    samples: Vec<Complex<f32>>,
    power: Vec<f32>,
    floor: Vec<f32>,
    candidates: Vec<(f32, f32)>,
    pending: Vec<Pending>,
    spots: Vec<CwSkimmerSpot>,
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

/// A hard-keyed station's key clicks carry its own keying across kilohertz of band, so a weaker
/// track can decode the same characters in the same instant. Two operators never do.
fn publish(spots: &mut Vec<CwSkimmerSpot>, out: &mut ChannelOutputs) {
    spots.sort_unstable_by(|left, right| right.snr_db.total_cmp(&left.snr_db));
    let mut kept = 0;
    for index in 0..spots.len() {
        let echo = spots[..kept].iter().any(|louder| {
            louder.text == spots[index].text
                && louder.snr_db - spots[index].snr_db >= ECHO_MARGIN_DB
        });
        if !echo {
            spots.swap(kept, index);
            kept += 1;
        }
    }
    spots.truncate(kept);
    for spot in spots.drain(..) {
        out.events.push(DecoderEvent::CwSkimmer(spot));
    }
}

fn pending_limit(max_signals: u16) -> usize {
    usize::from(max_signals) * 4
}

fn spot_limit(max_signals: u16) -> usize {
    usize::from(max_signals) * usize::from(PROVE_CHUNKS + 1)
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
        self.pending.clear();
        self.pending.reserve(pending_limit(self.max_signals));
        self.spots.reserve(spot_limit(self.max_signals));
        self.tracks.truncate(usize::from(self.max_signals));
        Ok(())
    }

    fn analyze(&mut self) {
        self.analyzer
            .power_db(&self.samples[..FFT_SIZE], &mut self.power);
        let bin_hz = RATE as f32 / FFT_SIZE as f32;
        let centre = FFT_SIZE / 2;
        let half_bins = (self.bandwidth_hz as f32 / 2.0 / bin_hz) as usize;
        let low = centre.saturating_sub(half_bins).max(2);
        let high = (centre + half_bins + 1).min(FFT_SIZE - 2);
        if high <= low {
            return;
        }
        self.noise.estimate(&self.power[low..high], &mut self.floor);
        if self.floor.len() != high - low {
            return;
        }
        self.candidates.clear();
        for index in low..high {
            let power = self.power[index];
            let floor = self.floor[index - low];
            if power >= floor + self.threshold_db
                && power > self.power[index - 1]
                && power >= self.power[index + 1]
                && power - self.power[index - 2].max(self.power[index + 2]) >= 3.0
            {
                self.candidates
                    .push(((index as f32 - centre as f32) * bin_hz, power - floor));
            }
        }
        self.candidates
            .sort_by(|left, right| right.1.total_cmp(&left.1));
        if let Some(&(_, loudest)) = self.candidates.first() {
            self.candidates
                .retain(|&(_, snr)| snr >= loudest - SPUR_RANGE_DB);
        }
        for track in &mut self.tracks {
            track.misses = track.misses.saturating_add(1);
            track.age = track.age.saturating_add(1);
        }
        for entry in &mut self.pending {
            entry.age = entry.age.saturating_add(1);
        }
        for index in 0..self.candidates.len() {
            let (frequency, snr) = self.candidates[index];
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
            if self
                .tracks
                .iter()
                .any(|track| (track.frequency_hz - frequency).abs() < TRACK_SEPARATION_HZ)
            {
                continue;
            }
            self.confirm(frequency, snr);
        }
        self.pending
            .retain(|entry| entry.hits < INIT_HITS && entry.age < INIT_FRAMES);
        self.tracks
            .retain(|track| track.misses < TRACK_MISSES && !track.spent());
    }

    /// A single frame's peak is as likely to be the noise the threshold lets through as a station,
    /// so a frequency has to keep showing up before it gets a decoder of its own.
    fn confirm(&mut self, frequency_hz: f32, snr_db: f32) {
        let matched = self
            .pending
            .iter_mut()
            .min_by(|left, right| {
                (left.frequency_hz - frequency_hz)
                    .abs()
                    .total_cmp(&(right.frequency_hz - frequency_hz).abs())
            })
            .filter(|entry| (entry.frequency_hz - frequency_hz).abs() <= MATCH_HZ);
        let entry = match matched {
            Some(entry) => {
                entry.frequency_hz = entry.frequency_hz * 0.5 + frequency_hz * 0.5;
                entry.snr_db = entry.snr_db.max(snr_db);
                entry.hits = entry.hits.saturating_add(1);
                entry
            }
            None => {
                if self.pending.len() >= pending_limit(self.max_signals) {
                    return;
                }
                self.pending.push(Pending {
                    frequency_hz,
                    snr_db,
                    hits: 1,
                    age: 0,
                });
                return;
            }
        };
        if entry.hits < INIT_HITS {
            return;
        }
        let (frequency_hz, snr_db) = (entry.frequency_hz, entry.snr_db);
        if self.tracks.len() < usize::from(self.max_signals) {
            self.tracks.push(self.prototype.spawn(frequency_hz, snr_db));
            return;
        }
        let weakest = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| !track.proven && track.snr_db < snr_db)
            .min_by(|(_, left), (_, right)| left.snr_db.total_cmp(&right.snr_db))
            .map(|(index, _)| index);
        if let Some(index) = weakest {
            self.tracks[index] = self.prototype.spawn(frequency_hz, snr_db);
        }
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
            noise: NoiseFloor::new(FLOOR_HALF_BINS, FLOOR_STRIDE_BINS),
            samples: Vec::with_capacity(FFT_SIZE + FFT_HOP),
            power: vec![0.0; FFT_SIZE],
            floor: Vec::with_capacity(FFT_SIZE),
            candidates: Vec::with_capacity(usize::from(params.max_signals) * 4),
            pending: Vec::with_capacity(pending_limit(params.max_signals)),
            spots: Vec::with_capacity(spot_limit(params.max_signals)),
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
        self.pending.clear();
        self.tracks.clear();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.spots.clear();
        for track in &mut self.tracks {
            track.process(iq, &mut self.spots);
        }
        publish(&mut self.spots, out);
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

    fn spots(channel: &mut CwSkimmerChannel, iq: &[Complex<f32>]) -> Vec<CwSkimmerSpot> {
        let mut out = ChannelOutputs::default();
        for chunk in iq.chunks(2_047) {
            channel.process(chunk, &mut out);
        }
        out.events
            .iter()
            .filter_map(|event| match event {
                DecoderEvent::CwSkimmer(spot) => Some(spot.clone()),
                _ => None,
            })
            .collect()
    }

    fn loud(mut iq: Vec<Complex<f32>>, snr_db: f32, seed: u32) -> Vec<Complex<f32>> {
        let gain = 10f32.powf(snr_db / 20.0);
        for sample in &mut iq {
            *sample *= gain;
        }
        testgen::add_noise(&mut iq, seed, 1.0);
        iq
    }

    #[test]
    fn a_hard_keyed_station_is_the_only_station_its_key_clicks_produce() {
        let iq = loud(
            testgen::morse::hard_keyed(
                "VVV VVV CQ DE DL1AAA K VVV VVV CQ DE DL1AAA K",
                20.0,
                3_500.0,
                RATE,
            ),
            55.0,
            7,
        );
        let mut channel = channel(CwSkimmerParams {
            bandwidth_hz: 16_000.0,
            ..CwSkimmerParams::default()
        })
        .unwrap();
        let all = spots(&mut channel, &iq);
        let heard: String = all
            .iter()
            .filter(|spot| (spot.offset_hz - 3_500.0).abs() <= 150.0)
            .map(|spot| spot.text.as_str())
            .collect();
        assert!(heard.contains("DL1AAA"), "{heard:?}");
        let elsewhere: Vec<(f32, &str)> = all
            .iter()
            .filter(|spot| (spot.offset_hz - 3_500.0).abs() > 150.0)
            .map(|spot| (spot.offset_hz, spot.text.as_str()))
            .collect();
        assert!(
            elsewhere.is_empty(),
            "stations that are not there: {elsewhere:?}"
        );
    }

    #[test]
    fn key_clicks_never_become_a_second_station_even_wide_open() {
        let iq = loud(
            testgen::morse::hard_keyed(
                "VVV VVV CQ DE DL1AAA K VVV VVV CQ DE DL1AAA K",
                20.0,
                3_500.0,
                RATE,
            ),
            55.0,
            19,
        );
        let mut channel = channel(CwSkimmerParams {
            bandwidth_hz: 16_000.0,
            threshold_db: 6.0,
            ..CwSkimmerParams::default()
        })
        .unwrap();
        let all = spots(&mut channel, &iq);
        let heard: String = all
            .iter()
            .filter(|spot| (spot.offset_hz - 3_500.0).abs() <= 150.0)
            .map(|spot| spot.text.as_str())
            .collect();
        assert!(heard.contains("DL1AAA"), "{heard:?}");
        let ghosts: Vec<(f32, &str)> = all
            .iter()
            .filter(|spot| (spot.offset_hz - 3_500.0).abs() > 150.0)
            .map(|spot| (spot.offset_hz, spot.text.as_str()))
            .collect();
        assert!(
            ghosts.is_empty(),
            "key clicks reported as stations: {ghosts:?}"
        );
    }

    #[test]
    fn two_stations_sending_the_same_words_are_both_reported() {
        let text = "CQ DE TEST K CQ DE TEST K";
        let mut iq = testgen::morse::transmission(text, 20.0, -2_000.0, RATE);
        for (destination, source) in iq
            .iter_mut()
            .zip(testgen::morse::transmission(text, 20.0, 2_000.0, RATE))
        {
            *destination += source;
        }
        let iq = loud(iq, 30.0, 5);
        let mut channel = channel(CwSkimmerParams {
            bandwidth_hz: 16_000.0,
            ..CwSkimmerParams::default()
        })
        .unwrap();
        let all = spots(&mut channel, &iq);
        for offset in [-2_000.0f32, 2_000.0] {
            let heard: String = all
                .iter()
                .filter(|spot| (spot.offset_hz - offset).abs() <= 150.0)
                .map(|spot| spot.text.as_str())
                .collect();
            assert!(heard.contains("TEST"), "at {offset} Hz: {heard:?}");
        }
    }

    #[test]
    fn a_real_station_takes_the_slot_a_noise_track_is_sitting_on() {
        let mut iq = loud(
            testgen::morse::transmission("CQ DE G4BBB K", 22.0, 2_000.0, RATE),
            25.0,
            5,
        );
        iq.extend(testgen::silence(RATE as usize));
        let mut channel = channel(CwSkimmerParams {
            bandwidth_hz: 16_000.0,
            threshold_db: 3.0,
            max_signals: 1,
            wpm: None,
        })
        .unwrap();
        let heard: String = spots(&mut channel, &iq)
            .iter()
            .map(|spot| spot.text.as_str())
            .collect();
        assert!(heard.contains("G4BBB"), "{heard:?}");
    }

    #[test]
    fn an_empty_band_is_reported_empty_even_at_the_loosest_threshold() {
        let iq = loud(vec![Complex::new(0.0, 0.0); RATE as usize * 20], 0.0, 3);
        let mut channel = channel(CwSkimmerParams {
            threshold_db: 3.0,
            ..CwSkimmerParams::default()
        })
        .unwrap();
        let all = spots(&mut channel, &iq);
        assert!(all.is_empty(), "noise decoded as {all:?}");
    }

    #[test]
    fn a_station_that_starts_after_the_band_was_quiet_still_gets_a_decoder() {
        let mut iq = vec![Complex::new(0.0, 0.0); RATE as usize * 10];
        iq.extend(testgen::morse::transmission(
            "CQ DE G4BBB K CQ DE G4BBB K",
            22.0,
            2_000.0,
            RATE,
        ));
        iq.extend(testgen::silence(RATE as usize * 2));
        let iq = loud(iq, 25.0, 11);
        let mut channel = channel(CwSkimmerParams::default()).unwrap();
        let heard: String = spots(&mut channel, &iq)
            .iter()
            .filter(|spot| (spot.offset_hz - 2_000.0).abs() <= 150.0)
            .map(|spot| spot.text.as_str())
            .collect();
        assert!(heard.contains("G4BBB"), "{heard:?}");
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
