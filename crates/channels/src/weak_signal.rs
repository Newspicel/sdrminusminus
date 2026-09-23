use std::{sync::LazyLock, thread, time::Duration};

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
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_dsp::{FirC, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DecoderFamily, WsjtMessage,
    WsjtParams, WsprParams, WsprSpot,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 12_000.0;
const FILTER_TAPS: usize = 257;
const MIN_AUDIO_HZ: f32 = 50.0;
const MAX_AUDIO_HZ: f32 = 5_500.0;
const QUEUED_SLOTS: usize = 2;
const SPARE_SLOTS: usize = 3;
const WORKER_IDLE: Duration = Duration::from_millis(2);
const WSPR_POWER_DBM: [i32; 19] = [
    0, 3, 7, 10, 13, 17, 20, 23, 27, 30, 33, 37, 40, 43, 47, 50, 53, 57, 60,
];

static FT8_DESCRIPTOR: LazyLock<ChannelDescriptor> =
    LazyLock::new(|| descriptor("ft8", "FT8", "FT8 weak signal contacts"));
static FT4_DESCRIPTOR: LazyLock<ChannelDescriptor> =
    LazyLock::new(|| descriptor("ft4", "FT4", "FT4 fast weak signal contacts"));
static WSPR_DESCRIPTOR: LazyLock<ChannelDescriptor> =
    LazyLock::new(|| descriptor("wspr", "WSPR", "WSPR propagation beacons"));

fn descriptor(type_id: &str, name: &str, summary: &str) -> ChannelDescriptor {
    ChannelDescriptor {
        type_id: type_id.to_owned(),
        name: name.to_owned(),
        summary: summary.to_owned(),
        family: DecoderFamily::Amateur,
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
            Self::Ft8 => 24_000,
            Self::Ft4 => 24_000,
            Self::Wspr => 60_000,
        }
    }

    fn type_id(self) -> &'static str {
        match self {
            Self::Ft8 => "ft8",
            Self::Ft4 => "ft4",
            Self::Wspr => "wspr",
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

struct Job {
    epoch: u64,
    window_start: u64,
    audio_low_hz: f32,
    audio_high_hz: f32,
    max_candidates: usize,
    samples: Vec<f32>,
}

struct Done {
    epoch: u64,
    window_start: u64,
    events: Vec<DecoderEvent>,
    samples: Vec<f32>,
}

struct WeakSignal {
    mode: Mode,
    audio_low_hz: f32,
    audio_high_hz: f32,
    max_candidates: usize,
    audio: Vec<f32>,
    window_start: u64,
    epoch: u64,
    queued: usize,
    recent: Vec<Recent>,
    jobs: Producer<Job>,
    done: Consumer<Done>,
    spare: Vec<Vec<f32>>,
}

impl WeakSignal {
    fn new(mode: Mode, settings: &ChannelSettings) -> Result<Self, ChannelError> {
        let (low, high, candidates) = configured(mode, settings)?;
        let (jobs, incoming) = RingBuffer::new(QUEUED_SLOTS);
        let (outgoing, done) = RingBuffer::new(QUEUED_SLOTS);
        thread::Builder::new()
            .name(format!("{}-decode", mode.type_id()))
            .spawn(move || run(mode, incoming, outgoing))
            .map_err(|error| {
                ChannelError::InvalidSettings(format!("Weak-signal worker: {error}"))
            })?;
        Ok(Self {
            mode,
            audio_low_hz: low,
            audio_high_hz: high,
            max_candidates: candidates,
            audio: Vec::with_capacity(mode.slot_samples() + mode.hop_samples()),
            window_start: 0,
            epoch: 0,
            queued: 0,
            recent: Vec::new(),
            jobs,
            done,
            spare: (0..SPARE_SLOTS)
                .map(|_| Vec::with_capacity(mode.slot_samples()))
                .collect(),
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
        self.window_start = 0;
        self.epoch = self.epoch.wrapping_add(1);
        self.recent.clear();
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
        let hop = self.mode.hop_samples();
        while self.audio.len() >= slot {
            self.submit(slot);
            self.audio.drain(..hop);
            self.window_start += hop as u64;
            let oldest = self.window_start.saturating_sub((2 * slot) as u64) as i64;
            self.recent.retain(|item| item.start_sample >= oldest);
        }
        self.collect(out);
    }

    fn submit(&mut self, slot: usize) {
        let Some(mut samples) = self.spare.pop() else {
            return;
        };
        samples.clear();
        samples.extend_from_slice(&self.audio[..slot]);
        let job = Job {
            epoch: self.epoch,
            window_start: self.window_start,
            audio_low_hz: self.audio_low_hz,
            audio_high_hz: self.audio_high_hz,
            max_candidates: self.max_candidates,
            samples,
        };
        match self.jobs.push(job) {
            Ok(()) => self.queued += 1,
            Err(PushError::Full(job)) => self.spare.push(job.samples),
        }
    }

    fn collect(&mut self, out: &mut ChannelOutputs) {
        while let Ok(done) = self.done.pop() {
            self.queued = self.queued.saturating_sub(1);
            if done.epoch == self.epoch {
                for event in done.events {
                    self.keep(done.window_start, event, out);
                }
            }
            self.spare.push(done.samples);
        }
    }

    fn keep(&mut self, window_start: u64, event: DecoderEvent, out: &mut ChannelOutputs) {
        let Some((text, audio_hz, time_offset_s)) = identity(&event) else {
            return;
        };
        let text = text.to_owned();
        let start = window_start as i64
            + ((self.mode.nominal_start_s() + f64::from(time_offset_s)) * INPUT_RATE_HZ).round()
                as i64;
        if self.is_duplicate(&text, audio_hz, start) {
            return;
        }
        out.events.push(event);
    }

    #[cfg(test)]
    fn settle(&mut self, out: &mut ChannelOutputs) {
        while self.queued > 0 {
            self.collect(out);
            thread::sleep(WORKER_IDLE);
        }
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

fn identity(event: &DecoderEvent) -> Option<(&str, f32, f32)> {
    match event {
        DecoderEvent::Ft8(message) | DecoderEvent::Ft4(message) => {
            Some((&message.text, message.audio_hz, message.time_offset_s))
        }
        DecoderEvent::Wspr(spot) => Some((&spot.text, spot.audio_hz, spot.time_offset_s)),
        _ => None,
    }
}

fn run(mode: Mode, mut input: Consumer<Job>, mut output: Producer<Done>) {
    let mut wspr_calls = WsprCallsignTable::new();
    let mut pcm = Vec::with_capacity(mode.slot_samples());
    loop {
        if output.is_abandoned() {
            return;
        }
        let Ok(job) = input.pop() else {
            if input.is_abandoned() {
                return;
            }
            thread::sleep(WORKER_IDLE);
            continue;
        };
        let events = match mode {
            Mode::Ft8 => decode_wsjt::<Ft8>(&job, &mut pcm, DecoderEvent::Ft8),
            Mode::Ft4 => decode_wsjt::<Ft4>(&job, &mut pcm, DecoderEvent::Ft4),
            Mode::Wspr => decode_wspr(&job, &mut wspr_calls),
        };
        let mut done = Done {
            epoch: job.epoch,
            window_start: job.window_start,
            events,
            samples: job.samples,
        };
        while let Err(PushError::Full(held)) = output.push(done) {
            if output.is_abandoned() {
                return;
            }
            done = held;
            thread::sleep(WORKER_IDLE);
        }
    }
}

fn decode_wsjt<P>(
    job: &Job,
    pcm: &mut Vec<i16>,
    event: fn(WsjtMessage) -> DecoderEvent,
) -> Vec<DecoderEvent>
where
    P: mfsk_core::msg::decode_request::FrameDecodable
        + mfsk_core::msg::decode_request::SupportsSicRounds<
            DecodeResult = mfsk_core::engine::pipeline::DecodeResult,
        >,
{
    pcm.clear();
    pcm.extend(
        job.samples
            .iter()
            .map(|sample| (sample * f32::from(i16::MAX)).round() as i16),
    );
    DecodeRequest::<P>::new(
        pcm,
        job.audio_low_hz,
        job.audio_high_hz,
        1.0,
        job.max_candidates,
    )
    .sic_rounds(2)
    .decode()
    .results
    .into_iter()
    .filter_map(|result| {
        let text = unpack77(result.message77())?;
        Some(event(WsjtMessage {
            text,
            snr_db: result.snr_db,
            audio_hz: result.freq_hz,
            time_offset_s: result.dt_sec,
            hard_errors: result.hard_errors,
        }))
    })
    .collect()
}

fn reported_by_a_transmitter(grid: Option<&str>, power_dbm: i32) -> bool {
    if !WSPR_POWER_DBM.contains(&power_dbm) {
        return false;
    }
    let Some(grid) = grid else {
        return true;
    };
    let locator = grid.as_bytes();
    if !matches!(locator.len(), 4 | 6) {
        return false;
    }
    let field = |byte: u8| (b'A'..=b'R').contains(&byte);
    let subsquare = |byte: u8| (b'A'..=b'X').contains(&byte);
    field(locator[0])
        && field(locator[1])
        && locator[2].is_ascii_digit()
        && locator[3].is_ascii_digit()
        && (locator.len() == 4 || (subsquare(locator[4]) && subsquare(locator[5])))
}

fn decode_wspr(job: &Job, calls: &mut WsprCallsignTable) -> Vec<DecoderEvent> {
    let params = SearchParams {
        freq_min_hz: job.audio_low_hz,
        freq_max_hz: job.audio_high_hz,
        max_candidates: job.max_candidates,
        ..SearchParams::default()
    };
    decode_scan_with_table(&job.samples, INPUT_RATE_HZ as u32, 0, &params, calls)
        .into_iter()
        .filter_map(|result| {
            let text = result.message.to_string();
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
            reported_by_a_transmitter(grid.as_deref(), power_dbm).then_some(DecoderEvent::Wspr(
                WsprSpot {
                    text,
                    callsign,
                    grid,
                    power_dbm,
                    snr_db: result.snr_db,
                    audio_hz: result.freq_hz,
                    time_offset_s: result.dt_sec,
                    drift_hz: result.drift_hz,
                },
            ))
        })
        .collect()
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
            frequency_hz: 0.0,
            squelch: sdrmm_wire::Squelch::Off,
            params: params.clone(),
            blanker: Default::default(),
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

        impl $name {
            #[cfg(test)]
            pub(crate) fn settle(&mut self, out: &mut ChannelOutputs) {
                self.0.settle(out);
            }
        }
    };
}

channel!(Ft8Channel, Mode::Ft8, FT8_DESCRIPTOR);
channel!(Ft4Channel, Mode::Ft4, FT4_DESCRIPTOR);
channel!(WsprChannel, Mode::Wspr, WSPR_DESCRIPTOR);

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::{
        testgen,
        testutil::{realtime_budget, settings},
    };

    macro_rules! decode {
        ($channel:expr, $iq:expr) => {{
            let mut channel = $channel;
            let mut out = ChannelOutputs::default();
            channel.process($iq, &mut out);
            channel.settle(&mut out);
            out.events
        }};
    }

    #[test]
    fn ft8_fixture_decodes_to_its_message_and_measurements() {
        let events = decode!(
            Ft8Channel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Ft8(WsjtParams::default())),
            )
            .unwrap(),
            &testgen::weak_signal::ft8_slot("W1AW", "FN42", 1_500.0)
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
        let events = decode!(
            Ft4Channel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Ft4(WsjtParams::default())),
            )
            .unwrap(),
            &testgen::weak_signal::ft4_slot("JA1ABC", "PM95", 1_000.0)
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
        let events = decode!(
            WsprChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Wspr(WsprParams::default())),
            )
            .unwrap(),
            &testgen::weak_signal::wspr_slot("K1ABC", "FN42", 37, 1_500.0)
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

    #[test]
    fn a_slot_decode_never_blocks_the_sample_path() {
        let iq = testgen::weak_signal::ft8_slot("W1AW", "FN42", 1_500.0);
        let mut channel = Ft8Channel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Ft8(WsjtParams::default())),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        let started = Instant::now();
        for block in iq.chunks(4_096) {
            channel.process(block, &mut out);
        }
        let spent = started.elapsed().as_secs_f64();
        let slot_seconds = iq.len() as f64 / INPUT_RATE_HZ;
        assert!(
            spent < realtime_budget(slot_seconds / 10.0),
            "feeding {slot_seconds:.1} s of audio held the sample path for {spent:.3} s"
        );
        channel.settle(&mut out);
        assert!(
            out.events.iter().any(|event| matches!(
                event,
                DecoderEvent::Ft8(message) if message.text.contains("W1AW")
            )),
            "{:?}",
            out.events
        );
    }

    #[test]
    fn a_spot_no_transmitter_could_have_sent_is_not_reported() {
        for (grid, power_dbm) in [
            (Some("A000AA"), 63),
            (Some("BWB9H "), 15),
            (Some("JN46"), 15),
            (Some("JN46!"), 30),
            (Some("SN46"), 30),
        ] {
            assert!(
                !reported_by_a_transmitter(grid, power_dbm),
                "{grid:?} at {power_dbm} dBm passed"
            );
        }
        for (grid, power_dbm) in [(Some("JN46"), 30), (Some("JN49GR"), 23), (None, 37)] {
            assert!(
                reported_by_a_transmitter(grid, power_dbm),
                "{grid:?} at {power_dbm} dBm was refused"
            );
        }
    }

    fn busy_slot_with_quiet_tail() -> Vec<Complex<f32>> {
        const FIXTURE: &[u8] = include_bytes!("../../../fixtures/ft8_20m_busy_12k.sigmf-data");
        let mut iq: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<8>()
            .0
            .iter()
            .map(|sample| {
                Complex::new(
                    f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
                    f32::from_le_bytes([sample[4], sample[5], sample[6], sample[7]]),
                )
            })
            .collect();
        iq.resize(iq.len() + 4 * INPUT_RATE_HZ as usize, Complex::default());
        iq
    }

    #[test]
    fn a_recorded_slot_reads_the_band_the_reference_decoder_published() {
        const PUBLISHED: [&str; 20] = [
            "VK4BLE OH8JK R-17",
            "RK6AH JH1AJT -05",
            "PA3EPP SP8NFO KN09",
            "RV6K RU3XL -13",
            "SQ8OHR UA9LL MO27",
            "ET3RFG/R IN3ADG -23",
            "CQ F4FSY JN25",
            "JR5MJS OH8NW 73",
            "SV1GN RK6AUV LN05",
            "PB5DX EI3CTB IO63",
            "CQ IZ1ANK JN33",
            "NT6Q OH8GDU -17",
            "CQ DL1UDO JO31",
            "VK4BLE OH1EDK -20",
            "CQ JA OH1LWZ KP11",
            "<...> ON7EE JO10",
            "CQ DG0OFT JO50",
            "CQ UB3AQS KO85",
            "PA3EPP SP8NFO KN09",
            "RV6K RU3XL -13",
        ];

        let mut channel = Ft8Channel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(ChannelParams::Ft8(WsjtParams::default())),
        )
        .unwrap();
        let mut out = ChannelOutputs::default();
        let mut texts = Vec::new();
        let take = |out: &mut ChannelOutputs, texts: &mut Vec<String>| {
            for event in out.events.drain(..) {
                let DecoderEvent::Ft8(message) = event else {
                    panic!("the FT8 channel emitted something else")
                };
                texts.push(message.text);
            }
        };
        for block in busy_slot_with_quiet_tail().chunks(4_096) {
            out.reset();
            channel.process(block, &mut out);
            take(&mut out, &mut texts);
        }
        channel.settle(&mut out);
        take(&mut out, &mut texts);

        let missing: Vec<&str> = PUBLISHED
            .iter()
            .filter(|wanted| !texts.iter().any(|text| text == *wanted))
            .copied()
            .collect();
        assert_eq!(
            missing,
            ["CQ UB3AQS KO85"],
            "the recording's published decodes changed, read {texts:?}"
        );
        assert!(
            texts.iter().all(|text| text.is_ascii() && !text.is_empty()),
            "{texts:?}"
        );
    }
}
