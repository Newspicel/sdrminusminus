use std::sync::LazyLock;

use mfsk_core::{
    ft4::Ft4,
    ft8::Ft8,
    msg::{WsprMessage as CoreWsprMessage, decode_request::DecodeRequest, wsjt77::unpack77},
    wspr::{
        decode::{WsprCallsignTable, decode_scan_with_table},
        search::SearchParams,
    },
};
use num_complex::Complex;
use sdrmm_dsp::{FirC, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, WsjtMessage, WsjtParams,
    WsprParams, WsprSpot,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 12_000.0;
const FILTER_TAPS: usize = 257;
const MIN_AUDIO_HZ: f32 = 50.0;
const MAX_AUDIO_HZ: f32 = 5_500.0;

static FT8_DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| descriptor("ft8", "FT8"));
static FT4_DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| descriptor("ft4", "FT4"));
static WSPR_DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| descriptor("wspr", "WSPR"));

fn descriptor(type_id: &str, name: &str) -> ChannelDescriptor {
    ChannelDescriptor {
        type_id: type_id.to_owned(),
        name: name.to_owned(),
        bandwidth_hz: 3_200.0,
        input_rate_hz: INPUT_RATE_HZ,
        has_audio: false,
        decoder_kind: Some(type_id.to_owned()),
        ..ChannelDescriptor::default()
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Ft8,
    Ft4,
    Wspr,
}

impl Mode {
    fn slot_samples(self) -> usize {
        match self {
            Self::Ft8 => 180_000,
            Self::Ft4 => 90_000,
            Self::Wspr => 1_440_000,
        }
    }

    fn hop_samples(self) -> usize {
        match self {
            // A frame fits wholly inside at least one window even when the receiver was opened
            // between UTC slot boundaries. The decoder then finds its exact start from sync.
            Self::Ft8 => 24_000,
            Self::Ft4 => 24_000,
            Self::Wspr => 60_000,
        }
    }

    fn nominal_start_s(self) -> f64 {
        match self {
            Self::Ft8 | Self::Ft4 => 0.5,
            Self::Wspr => 1.0,
        }
    }
}

#[derive(Clone)]
struct Recent {
    text: String,
    audio_hz: f32,
    start_sample: i64,
}

struct WeakSignal {
    mode: Mode,
    audio_low_hz: f32,
    audio_high_hz: f32,
    max_candidates: usize,
    audio: Vec<f32>,
    pcm: Vec<i16>,
    window_start: u64,
    recent: Vec<Recent>,
    wspr_calls: WsprCallsignTable,
}

impl WeakSignal {
    fn new(mode: Mode, settings: &ChannelSettings) -> Result<Self, ChannelError> {
        let (low, high, candidates) = configured(mode, settings)?;
        let audio = Vec::with_capacity(mode.slot_samples() + mode.hop_samples());
        let pcm = Vec::with_capacity(mode.slot_samples());
        Ok(Self {
            mode,
            audio_low_hz: low,
            audio_high_hz: high,
            max_candidates: candidates,
            audio,
            pcm,
            window_start: 0,
            recent: Vec::new(),
            wspr_calls: WsprCallsignTable::new(),
        })
    }

    fn apply(&mut self, settings: &ChannelSettings) -> Result<(), ChannelError> {
        let (low, high, candidates) = configured(self.mode, settings)?;
        self.audio_low_hz = low;
        self.audio_high_hz = high;
        self.max_candidates = candidates;
        Ok(())
    }

    fn reset(&mut self) {
        self.audio.clear();
        self.pcm.clear();
        self.window_start = 0;
        self.recent.clear();
        self.wspr_calls = WsprCallsignTable::new();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.audio.extend(iq.iter().map(|sample| {
            if sample.re.is_finite() {
                sample.re.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }));

        let slot = self.mode.slot_samples();
        while self.audio.len() >= slot {
            match self.mode {
                Mode::Ft8 => self.decode_wsjt::<Ft8>(out, DecoderEvent::Ft8),
                Mode::Ft4 => self.decode_wsjt::<Ft4>(out, DecoderEvent::Ft4),
                Mode::Wspr => self.decode_wspr(out),
            }
            let hop = self.mode.hop_samples();
            self.audio.drain(..hop);
            self.window_start += hop as u64;
            let oldest = self.window_start.saturating_sub((2 * slot) as u64) as i64;
            self.recent.retain(|item| item.start_sample >= oldest);
        }
    }

    fn decode_wsjt<P>(&mut self, out: &mut ChannelOutputs, event: fn(WsjtMessage) -> DecoderEvent)
    where
        P: mfsk_core::msg::decode_request::FrameDecodable
            + mfsk_core::msg::decode_request::SupportsSicRounds<
                DecodeResult = mfsk_core::engine::pipeline::DecodeResult,
            >,
    {
        self.pcm.clear();
        self.pcm.extend(
            self.audio
                .iter()
                .take(self.mode.slot_samples())
                .map(|sample| (sample * f32::from(i16::MAX)).round() as i16),
        );
        let results = DecodeRequest::<P>::new(
            &self.pcm,
            self.audio_low_hz,
            self.audio_high_hz,
            1.0,
            self.max_candidates,
        )
        .sic_rounds(2)
        .decode()
        .results;

        for result in results {
            let Some(text) = unpack77(result.message77()) else {
                continue;
            };
            let start = self.estimated_start(result.dt_sec);
            if self.is_duplicate(&text, result.freq_hz, start) {
                continue;
            }
            out.events.push(event(WsjtMessage {
                text,
                snr_db: result.snr_db,
                audio_hz: result.freq_hz,
                time_offset_s: result.dt_sec,
                hard_errors: result.hard_errors,
            }));
        }
    }

    fn decode_wspr(&mut self, out: &mut ChannelOutputs) {
        let params = SearchParams {
            freq_min_hz: self.audio_low_hz,
            freq_max_hz: self.audio_high_hz,
            max_candidates: self.max_candidates,
            ..SearchParams::default()
        };
        let results = decode_scan_with_table(
            &self.audio[..self.mode.slot_samples()],
            INPUT_RATE_HZ as u32,
            0,
            &params,
            &mut self.wspr_calls,
        );
        for result in results {
            let text = result.message.to_string();
            let start = self.estimated_start(result.dt_sec);
            if self.is_duplicate(&text, result.freq_hz, start) {
                continue;
            }
            let (callsign, grid, power_dbm) = match result.message {
                CoreWsprMessage::Type1 {
                    callsign,
                    grid,
                    power_dbm,
                } => (callsign, Some(grid), power_dbm),
                CoreWsprMessage::Type2 {
                    callsign,
                    power_dbm,
                } => (callsign, None, power_dbm),
                CoreWsprMessage::Type3 {
                    callsign_hash,
                    grid6,
                    power_dbm,
                } => (format!("<#{callsign_hash:05x}>"), Some(grid6), power_dbm),
            };
            out.events.push(DecoderEvent::Wspr(WsprSpot {
                text,
                callsign,
                grid,
                power_dbm,
                snr_db: result.snr_db,
                audio_hz: result.freq_hz,
                time_offset_s: result.dt_sec,
                drift_hz: result.drift_hz,
            }));
        }
    }

    fn estimated_start(&self, dt_sec: f32) -> i64 {
        self.window_start as i64
            + ((self.mode.nominal_start_s() + f64::from(dt_sec)) * INPUT_RATE_HZ).round() as i64
    }

    fn is_duplicate(&mut self, text: &str, audio_hz: f32, start_sample: i64) -> bool {
        let duplicate = self.recent.iter().any(|item| {
            item.text == text
                && (item.audio_hz - audio_hz).abs() < 3.0
                && (item.start_sample - start_sample).unsigned_abs() < INPUT_RATE_HZ as u64
        });
        if !duplicate {
            self.recent.push(Recent {
                text: text.to_owned(),
                audio_hz,
                start_sample,
            });
        }
        duplicate
    }
}

fn configured(mode: Mode, settings: &ChannelSettings) -> Result<(f32, f32, usize), ChannelError> {
    let (low, high, candidates) = match (mode, &settings.params) {
        (Mode::Ft8, ChannelParams::Ft8(p)) | (Mode::Ft4, ChannelParams::Ft4(p)) => wsjt_config(p),
        (Mode::Wspr, ChannelParams::Wspr(p)) => wspr_config(p),
        (_, other) => {
            return Err(ChannelError::InvalidSettings(format!(
                "weak-signal channel got {} params",
                other.type_id()
            )));
        }
    };
    validate_passband(low, high, candidates)?;
    Ok((low, high, candidates))
}

fn wsjt_config(params: &WsjtParams) -> (f32, f32, usize) {
    (
        params.audio_low_hz,
        params.audio_high_hz,
        usize::from(params.max_candidates),
    )
}

fn wspr_config(params: &WsprParams) -> (f32, f32, usize) {
    (
        params.audio_low_hz,
        params.audio_high_hz,
        usize::from(params.max_candidates),
    )
}

fn validate_passband(low: f32, high: f32, candidates: usize) -> Result<(), ChannelError> {
    if !(low.is_finite()
        && high.is_finite()
        && (MIN_AUDIO_HZ..high).contains(&low)
        && high <= MAX_AUDIO_HZ)
    {
        return Err(ChannelError::InvalidSettings(format!(
            "audio search band must satisfy {MIN_AUDIO_HZ} <= low < high <= {MAX_AUDIO_HZ} Hz, got {low}..{high}"
        )));
    }
    if candidates == 0 {
        return Err(ChannelError::InvalidSettings(
            "max_candidates must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn occupied_band(params: &ChannelParams) -> (f64, f64) {
    match params {
        ChannelParams::Ft8(p) | ChannelParams::Ft4(p) => {
            (f64::from(p.audio_low_hz), f64::from(p.audio_high_hz))
        }
        ChannelParams::Wspr(p) => (f64::from(p.audio_low_hz), f64::from(p.audio_high_hz)),
        _ => (f64::from(MIN_AUDIO_HZ), f64::from(MAX_AUDIO_HZ)),
    }
}

pub(crate) fn channel_filter(params: &ChannelParams) -> Result<ChannelFilter, ChannelError> {
    configured(
        match params {
            ChannelParams::Ft8(_) => Mode::Ft8,
            ChannelParams::Ft4(_) => Mode::Ft4,
            ChannelParams::Wspr(_) => Mode::Wspr,
            other => {
                return Err(ChannelError::InvalidSettings(format!(
                    "weak-signal filter got {} params",
                    other.type_id()
                )));
            }
        },
        &ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: params.clone(),
            audio: sdrmm_wire::AudioProcessing::default(),
        },
    )?;
    let (low, high) = occupied_band(params);
    let half_width = (high - low) / 2.0 / INPUT_RATE_HZ;
    let center = (high + low) / 2.0 / INPUT_RATE_HZ;
    Ok(ChannelFilter::Sideband(FirC::from_lowpass(
        &design_lowpass(FILTER_TAPS, half_width),
        center,
    )))
}

macro_rules! channel {
    ($name:ident, $mode:expr, $descriptor:ident) => {
        pub struct $name(WeakSignal);

        impl ChannelRx for $name {
            fn descriptor() -> &'static ChannelDescriptor {
                &$descriptor
            }

            fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
                check_input_rate(ctx, &$descriptor)?;
                Ok(Self(WeakSignal::new($mode, &settings)?))
            }

            fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
                self.0.apply(&settings)
            }

            fn retuned(&mut self) {
                self.0.reset();
            }

            fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
                self.0.process(iq, out);
            }
        }
    };
}

channel!(Ft8Channel, Mode::Ft8, FT8_DESCRIPTOR);
channel!(Ft4Channel, Mode::Ft4, FT4_DESCRIPTOR);
channel!(WsprChannel, Mode::Wspr, WSPR_DESCRIPTOR);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testgen, testutil::settings};

    fn decode<C: ChannelRx>(mut channel: C, iq: &[Complex<f32>]) -> Vec<DecoderEvent> {
        let mut out = ChannelOutputs::default();
        channel.process(iq, &mut out);
        out.events
    }

    #[test]
    fn ft8_fixture_decodes_to_its_message_and_measurements() {
        let events = decode(
            Ft8Channel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Ft8(WsjtParams::default())),
            )
            .unwrap(),
            &testgen::weak_signal::ft8_slot("W1AW", "FN42", 1_500.0),
        );
        let message = events.iter().find_map(|event| match event {
            DecoderEvent::Ft8(message) if message.text.contains("W1AW") => Some(message),
            _ => None,
        });
        let message = message.expect("FT8 fixture decode");
        assert!((message.audio_hz - 1_500.0).abs() < 10.0);
        assert!(message.time_offset_s.abs() < 0.2);
    }

    #[test]
    fn ft4_fixture_decodes_to_its_message_and_measurements() {
        let events = decode(
            Ft4Channel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Ft4(WsjtParams::default())),
            )
            .unwrap(),
            &testgen::weak_signal::ft4_slot("JA1ABC", "PM95", 1_000.0),
        );
        let message = events.iter().find_map(|event| match event {
            DecoderEvent::Ft4(message) if message.text.contains("JA1ABC") => Some(message),
            _ => None,
        });
        let message = message.expect("FT4 fixture decode");
        assert!((message.audio_hz - 1_000.0).abs() < 20.0);
        assert!(message.time_offset_s.abs() < 0.2);
    }

    #[test]
    fn wspr_fixture_decodes_to_a_spot() {
        let events = decode(
            WsprChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Wspr(WsprParams::default())),
            )
            .unwrap(),
            &testgen::weak_signal::wspr_slot("K1ABC", "FN42", 37, 1_500.0),
        );
        let spot = events.iter().find_map(|event| match event {
            DecoderEvent::Wspr(spot) if spot.callsign == "K1ABC" => Some(spot),
            _ => None,
        });
        let spot = spot.expect("WSPR fixture decode");
        assert_eq!(spot.grid.as_deref(), Some("FN42"));
        assert_eq!(spot.power_dbm, 37);
        assert!((spot.audio_hz - 1_500.0).abs() < 3.0);
    }
}
