use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sdrmm_device::{
    DeviceError,
    net::{Endpoint, Incoming, WebSocket},
};

use crate::proto::{Command, Field, Refusal, fields};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);
const RATE_TOLERANCE: f64 = 1e-3;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Station {
    pub sample_rate: f64,
    pub low_hz: f64,
    pub span_hz: f64,
    pub channels: Option<u32>,
}

impl Station {
    pub(crate) fn high_hz(&self) -> f64 {
        self.low_hz + self.span_hz
    }

    pub(crate) fn half_width_hz(&self) -> u32 {
        (self.sample_rate / 2.0).floor() as u32
    }

    pub(crate) fn khz(&self, center_hz: f64) -> f64 {
        (center_hz - self.low_hz) / 1000.0
    }

    pub(crate) fn same_as(&self, other: &Self) -> bool {
        (self.sample_rate - other.sample_rate).abs() <= self.sample_rate * RATE_TOLERANCE
            && self.low_hz == other.low_hz
            && self.span_hz == other.span_hz
    }
}

#[derive(Debug, Default)]
struct Heard {
    sample_rate: Option<f64>,
    audio_rate: Option<u32>,
    span_hz: Option<f64>,
    offset_khz: Option<f64>,
    channels: Option<u32>,
}

impl Heard {
    fn note(&mut self, field: Field) -> Result<(), DeviceError> {
        match field {
            Field::SampleRate(rate) => self.sample_rate = Some(rate),
            Field::AudioRate(rate) => self.audio_rate = Some(rate),
            Field::Span(hz) => self.span_hz = Some(hz),
            Field::OffsetKhz(khz) => self.offset_khz = Some(khz),
            Field::Channels(count) => self.channels = Some(count),
            Field::Refused(refusal) => return Err(refused(&refusal)),
        }
        Ok(())
    }

    fn station(&self) -> Result<Option<Station>, DeviceError> {
        let (Some(sample_rate), Some(_), Some(span_hz)) =
            (self.sample_rate, self.audio_rate, self.span_hz)
        else {
            return Ok(None);
        };
        if !(sample_rate.is_finite() && sample_rate >= 2.0) {
            return Err(DeviceError::Io(format!(
                "the KiwiSDR reported a sample rate of {sample_rate}"
            )));
        }
        if !(span_hz.is_finite() && span_hz > 0.0) {
            return Err(DeviceError::Io(format!(
                "the KiwiSDR reported a span of {span_hz} Hz"
            )));
        }
        Ok(Some(Station {
            sample_rate,
            low_hz: self.offset_khz.unwrap_or(0.0) * 1000.0,
            span_hz,
            channels: self.channels,
        }))
    }
}

pub(crate) fn refused(refusal: &Refusal) -> DeviceError {
    let reason = refusal.reason();
    match refusal {
        Refusal::Busy(_) | Refusal::Password(5) => DeviceError::InUse(reason),
        Refusal::Password(_) => DeviceError::PermissionDenied(reason),
        _ => DeviceError::Disconnected(reason),
    }
}

pub(crate) fn connect(endpoint: &Endpoint) -> Result<WebSocket, DeviceError> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    WebSocket::connect(endpoint, &format!("/kiwi/{stamp}/SND"))
}

pub(crate) fn send(socket: &WebSocket, commands: &[Command]) -> Result<(), DeviceError> {
    commands
        .iter()
        .try_for_each(|command| socket.send_text(&command.encode()))
}

pub(crate) fn login(socket: &WebSocket, password: &str) -> Result<Station, DeviceError> {
    send(socket, &[Command::Login(password.to_string())])?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut heard = Heard::default();
    let mut acked = false;
    while Instant::now() < deadline {
        let frame = match socket.next(POLL) {
            Incoming::Binary(block) => block.to_vec(),
            Incoming::Text(text) => text.into_bytes(),
            Incoming::Idle => continue,
            Incoming::Ended => {
                return Err(DeviceError::Io(format!(
                    "the KiwiSDR login: {}",
                    socket.failure().reason
                )));
            }
        };
        for field in fields(&frame) {
            heard.note(field)?;
        }
        if let (false, Some(rate)) = (acked, heard.audio_rate) {
            send(socket, &[Command::AudioRate(rate)])?;
            acked = true;
        }
        if let Some(station) = heard.station()? {
            return Ok(station);
        }
    }
    Err(DeviceError::Io(format!(
        "the KiwiSDR did not finish its login within {} s",
        LOGIN_TIMEOUT.as_secs()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station() -> Station {
        Station {
            sample_rate: 11_998.924,
            low_hz: 0.0,
            span_hz: 30e6,
            channels: Some(8),
        }
    }

    #[test]
    fn the_station_needs_rate_audio_rate_and_span() {
        let mut heard = Heard::default();
        heard.note(Field::SampleRate(11_998.924)).expect("noted");
        heard.note(Field::Span(30e6)).expect("noted");
        assert_eq!(heard.station().expect("valid"), None);
        heard.note(Field::AudioRate(12_000)).expect("noted");
        heard.note(Field::Channels(8)).expect("noted");
        assert_eq!(heard.station().expect("valid"), Some(station()));
    }

    #[test]
    fn a_transverter_offset_moves_the_band() {
        let mut heard = Heard::default();
        for field in [
            Field::SampleRate(12_000.0),
            Field::AudioRate(12_000),
            Field::Span(30e6),
            Field::OffsetKhz(100_000.0),
        ] {
            heard.note(field).expect("noted");
        }
        let station = heard.station().expect("valid").expect("complete");
        assert_eq!(station.low_hz, 100e6);
        assert_eq!(station.high_hz(), 130e6);
        assert_eq!(station.khz(107_074_000.0), 7_074.0);
    }

    #[test]
    fn nonsense_rates_are_refused() {
        let mut heard = Heard::default();
        for field in [
            Field::SampleRate(0.0),
            Field::AudioRate(12_000),
            Field::Span(30e6),
        ] {
            heard.note(field).expect("noted");
        }
        assert!(heard.station().is_err());
    }

    #[test]
    fn a_refusal_ends_the_login_with_a_matching_error() {
        let mut heard = Heard::default();
        assert!(matches!(
            heard.note(Field::Refused(Refusal::Busy(4))),
            Err(DeviceError::InUse(reason)) if reason.contains('4')
        ));
        assert!(matches!(
            refused(&Refusal::Password(1)),
            DeviceError::PermissionDenied(_)
        ));
        assert!(matches!(
            refused(&Refusal::Kicked),
            DeviceError::Disconnected(_)
        ));
    }

    #[test]
    fn a_drifting_gps_rate_is_still_the_same_station() {
        let drifted = Station {
            sample_rate: station().sample_rate * 1.000_01,
            ..station()
        };
        assert!(station().same_as(&drifted));
        let wide = Station {
            sample_rate: 20_250.0,
            ..station()
        };
        assert!(!station().same_as(&wide));
        assert_eq!(station().half_width_hz(), 5_999);
    }
}
