mod decoder;
mod detect;

use std::collections::VecDeque;

use decoder::Decoder;
use num_complex::Complex;
use sdrmm_wire::{DecoderEvent, IdentSignal, SpectrumMonitorNode, Transmission, TransmissionState};

use crate::{ChannelError, ident};

const MAX_TRACKS: usize = 32;
const MAX_HISTORY_SAMPLES: usize = 8 * 1024 * 1024;
const HANG_SECONDS: f64 = 0.3;
const SEGMENT_SECONDS: f64 = 30.0;

pub struct MonitorOutput {
    pub transmission: u64,
    pub frequency_hz: f64,
    pub event: DecoderEvent,
    pub audio: Vec<i16>,
}

struct Track {
    id: u64,
    signal: IdentSignal,
    segment: u64,
    last_seen: u64,
    decoders: Vec<Decoder>,
    fallback: Option<Decoder>,
    choices: VecDeque<String>,
    trial_at: u64,
    identified_at: u64,
    band: ident::detect::Band,
    tried: Vec<String>,
    error: Option<String>,
}

pub struct SpectrumMonitor {
    settings: SpectrumMonitorNode,
    rate: f64,
    center: f64,
    next: Option<u64>,
    next_id: u64,
    window: Vec<Complex<f32>>,
    history: VecDeque<Complex<f32>>,
    history_capacity: usize,
    detector: detect::Detector,
    tracks: Vec<Track>,
    overloaded: bool,
}

impl SpectrumMonitor {
    pub fn new(
        rate: f64,
        center: f64,
        settings: SpectrumMonitorNode,
    ) -> Result<Self, ChannelError> {
        if !settings.valid() {
            return Err(ChannelError::InvalidSettings(
                "monitor confidence must be between 0 and 1".to_owned(),
            ));
        }
        if !rate.is_finite() || !(8000.0..=64_000_000.0).contains(&rate) || !center.is_finite() {
            return Err(ChannelError::InvalidSettings(
                "monitor requires 8 kHz to 64 MHz IQ and a finite center".to_owned(),
            ));
        }
        let history_capacity = ((rate * 2.0) as usize).clamp(4096, MAX_HISTORY_SAMPLES);
        Ok(Self {
            settings,
            rate,
            center,
            next: None,
            next_id: 1,
            window: Vec::with_capacity((rate * 0.1) as usize),
            history: VecDeque::with_capacity(history_capacity),
            history_capacity,
            detector: detect::Detector::new(rate),
            tracks: Vec::new(),
            overloaded: false,
        })
    }

    pub fn process(&mut self, iq: &[Complex<f32>], start: u64) -> Vec<MonitorOutput> {
        let mut output = Vec::new();
        if self.next.is_some_and(|next| next != start) {
            output.extend(self.finish(TransmissionState::Interrupted, Some("IQ samples lost")));
            output.push(self.problem("IQ samples lost", start));
            self.history.clear();
            self.window.clear();
        }
        let window_len = ((self.rate * 0.1) as usize).max(4096);
        let mut position = start;
        let mut remaining = iq;
        while !remaining.is_empty() {
            let take = (window_len - self.window.len()).min(remaining.len());
            let samples = &remaining[..take];
            for track in &mut self.tracks {
                feed(track, samples, &mut output);
            }
            self.window.extend_from_slice(samples);
            let excess = (self.history.len() + samples.len()).saturating_sub(self.history_capacity);
            self.history.drain(..excess.min(self.history.len()));
            self.history.extend(samples.iter().copied());
            position += take as u64;
            if self.window.len() == window_len {
                self.survey(position, &mut output);
                self.window.clear();
            }
            remaining = &remaining[take..];
        }
        self.next = Some(position);
        output
    }

    fn survey(&mut self, end: u64, output: &mut Vec<MonitorOutput>) {
        let bands = self.detector.measure(&self.window);
        let mut overflow = false;
        for band in bands {
            let frequency = self.center + band.center_hz;
            if let Some(track) = self.tracks.iter_mut().find(|track| {
                (track.signal.frequency_hz - frequency).abs()
                    < ((track.signal.bandwidth_hz + band.bandwidth_hz) * 0.4).max(1500.0)
            }) {
                track.last_seen = end;
                track.signal.snr_db = band.snr_db;
                if band.bandwidth_hz >= track.band.bandwidth_hz * 0.75 {
                    let offset = track.band.center_hz * 0.9 + band.center_hz * 0.1;
                    for decoder in track.decoders.iter_mut().chain(track.fallback.iter_mut()) {
                        decoder.retune(offset);
                    }
                    track.band = ident::detect::Band {
                        center_hz: offset,
                        bandwidth_hz: track.band.bandwidth_hz.max(band.bandwidth_hz),
                        ..band
                    };
                    track.signal.center_offset_hz = offset;
                    track.signal.frequency_hz = self.center + offset;
                }
                continue;
            }
            let signal = ident::identify(&self.window, self.rate, &band, self.center);
            if signal.confidence < self.settings.min_confidence {
                continue;
            }
            if self.tracks.len() >= MAX_TRACKS {
                overflow = true;
                continue;
            }
            let start = end.saturating_sub(self.history.len() as u64);
            let mut track = Track {
                id: self.next_id,
                choices: decoder::choices(&signal).into(),
                signal,
                segment: start,
                last_seen: end,
                decoders: Vec::new(),
                fallback: None,
                trial_at: end,
                identified_at: end,
                band,
                tried: Vec::new(),
                error: None,
            };
            self.next_id += 1;
            if matches!(
                track.signal.modulation,
                sdrmm_wire::Modulation::Fsk2
                    | sdrmm_wire::Modulation::Fsk4
                    | sdrmm_wire::Modulation::Fsk8
            ) {
                match Decoder::new("nfm", self.rate, &track.signal, self.settings.record_audio) {
                    Ok(decoder) => track.fallback = Some(decoder),
                    Err(error) => track.error = Some(error.to_string()),
                }
            }
            self.start_trials(&mut track);
            let buffered = self.history.make_contiguous();
            feed(&mut track, buffered, output);
            self.tracks.push(track);
        }
        if overflow && !self.overloaded {
            output.push(self.problem(
                "32 simultaneous signals reached; additional signals skipped",
                end,
            ));
        }
        self.overloaded = overflow;
        let mut tracks = std::mem::take(&mut self.tracks);
        for mut track in tracks.drain(..) {
            if end.saturating_sub(track.last_seen) as f64 >= self.rate * HANG_SECONDS
                || !self.refresh(&mut track, end, output)
            {
                output.push(transmission(
                    &mut track,
                    end,
                    self.rate,
                    TransmissionState::Completed,
                ));
                continue;
            }
            if end.saturating_sub(track.segment) as f64 >= self.rate * SEGMENT_SECONDS {
                output.push(transmission(
                    &mut track,
                    end,
                    self.rate,
                    TransmissionState::Continued,
                ));
                track.segment = end;
            }
            self.tracks.push(track);
        }
    }

    fn refresh(&mut self, track: &mut Track, end: u64, output: &mut Vec<MonitorOutput>) -> bool {
        if track.decoders.iter().any(|decoder| decoder.verified) {
            return true;
        }
        if end == track.last_seen
            && end.saturating_sub(track.identified_at) as f64 >= self.rate * 0.5
        {
            track.identified_at = end;
            let signal = ident::identify(&self.window, self.rate, &track.band, self.center);
            if signal.confidence < self.settings.min_confidence {
                return false;
            }
            for kind in decoder::choices(&signal) {
                if !track.tried.contains(&kind)
                    && !track.choices.contains(&kind)
                    && !track
                        .fallback
                        .as_ref()
                        .is_some_and(|fallback| fallback.kind == kind)
                {
                    track.choices.push_back(kind);
                }
            }
            track.signal = signal;
        }
        if end.saturating_sub(track.trial_at) as f64 >= self.rate * 2.0 && !track.choices.is_empty()
        {
            track.decoders.retain(|decoder| decoder.confirmed);
            track.trial_at = end;
        }
        let first = track.decoders.len();
        self.start_trials(track);
        if track.decoders.len() > first
            && end.saturating_sub(self.history.len() as u64) > track.segment
        {
            track.error = Some("decoder retry starts after retained IQ".to_owned());
        }
        for decoder in &mut track.decoders[first..] {
            let (events, error) = decoder.process(self.history.make_contiguous());
            if error.is_some() {
                track.error = error;
            }
            for event in events {
                output.push(MonitorOutput {
                    transmission: track.id,
                    frequency_hz: track.signal.frequency_hz,
                    event,
                    audio: Vec::new(),
                });
            }
        }
        true
    }

    fn start_trials(&self, track: &mut Track) {
        for _ in track.decoders.len()..3 {
            let Some(kind) = track.choices.pop_front() else {
                break;
            };
            track.tried.push(kind.clone());
            match Decoder::new(&kind, self.rate, &track.signal, self.settings.record_audio) {
                Ok(decoder) => track.decoders.push(decoder),
                Err(error) => track.error = Some(error.to_string()),
            }
        }
    }

    pub fn finish(&mut self, state: TransmissionState, error: Option<&str>) -> Vec<MonitorOutput> {
        let end = self.next.unwrap_or(0);
        let mut output = Vec::new();
        if !self.window.is_empty() {
            self.survey(end, &mut output);
            self.window.clear();
        }
        output.extend(self.tracks.drain(..).map(|mut track| {
            if let Some(error) = error {
                track.error = Some(error.to_owned());
            }
            transmission(&mut track, end, self.rate, state)
        }));
        output
    }

    fn problem(&self, error: &str, at: u64) -> MonitorOutput {
        MonitorOutput {
            transmission: 0,
            frequency_hz: self.center,
            audio: Vec::new(),
            event: DecoderEvent::Transmission(Transmission {
                id: 0,
                state: TransmissionState::Problem,
                signal: IdentSignal::default(),
                start_sample: at,
                end_sample: at,
                sample_rate_hz: self.rate,
                duration_ms: 0,
                started_at: None,
                ended_at: None,
                decoder: None,
                decoder_confirmed: false,
                audio: None,
                error: Some(error.to_owned()),
            }),
        }
    }
}

fn feed(track: &mut Track, iq: &[Complex<f32>], output: &mut Vec<MonitorOutput>) {
    if let Some(fallback) = &mut track.fallback {
        let (_, error) = fallback.process(iq);
        if error.is_some() {
            track.error = error;
        }
    }
    for decoder in &mut track.decoders {
        let (events, error) = decoder.process(iq);
        if error.is_some() {
            track.error = error;
        }
        for event in events {
            output.push(MonitorOutput {
                transmission: track.id,
                frequency_hz: track.signal.frequency_hz,
                event,
                audio: Vec::new(),
            });
        }
    }
    if let Some(chosen) = track.decoders.iter().position(|decoder| decoder.verified) {
        let decoder = track.decoders.remove(chosen);
        track.decoders.clear();
        track.decoders.push(decoder);
        track.choices.clear();
        track.fallback = None;
    }
}

fn transmission(track: &mut Track, end: u64, rate: f64, state: TransmissionState) -> MonitorOutput {
    let selected = track
        .decoders
        .iter_mut()
        .find(|decoder| decoder.confirmed)
        .or(track.fallback.as_mut());
    let decoder_confirmed = selected.as_ref().is_some_and(|decoder| decoder.verified);
    let (decoder, audio) = selected.map_or((None, Vec::new()), |decoder| {
        (
            Some(decoder.kind.clone()),
            std::mem::take(&mut decoder.audio),
        )
    });
    for decoder in track.decoders.iter_mut().chain(track.fallback.iter_mut()) {
        decoder.audio.clear();
    }
    MonitorOutput {
        transmission: track.id,
        frequency_hz: track.signal.frequency_hz,
        audio,
        event: DecoderEvent::Transmission(Transmission {
            id: track.id,
            state,
            signal: track.signal.clone(),
            start_sample: track.segment,
            end_sample: end,
            sample_rate_hz: rate,
            duration_ms: ((end.saturating_sub(track.segment)) as f64 * 1000.0 / rate) as u64,
            started_at: None,
            ended_at: None,
            decoder,
            decoder_confirmed,
            audio: None,
            error: track.error.clone(),
        }),
    }
}

#[cfg(test)]
mod tests;
