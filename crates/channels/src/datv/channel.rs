use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FracResampler, design_lowpass, flat_bandwidth_hz};
use sdrmm_modem::{
    constellation::tables,
    linear::{CarrierLoop, LinearDemod, LinearParams, LinearTiming, PhaseDetector},
    pulse::{self, Norm},
};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, BroadcastStatus, BroadcastSystem, ChannelDescriptor,
    ChannelParams, ChannelSettings, DatvParams, DatvStandard, DecoderEvent,
};

use super::{
    acquire::{Acquired, Acquisition},
    dvbs::{DvbsDecoder, PACKET},
    dvbs2::receiver::Dvbs2Decoder,
    ts::{PesUnit, TsDemux},
};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 2_000_000.0;
const BANDWIDTH_HZ: f64 = 1_500_000.0;
const MIN_SYMBOL_RATE: f64 = 100_000.0;
const MAX_SYMBOL_RATE: f64 = 1_000_000.0;
const ROLL_OFF: f64 = 0.35;
const SPS: usize = 4;
const PULSE_SPAN: usize = 8;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "datv".to_owned(),
    name: "DATV (DVB-S / S2)".to_owned(),
    bandwidth_hz: BANDWIDTH_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

fn params(settings: &ChannelSettings) -> Result<DatvParams, ChannelError> {
    match settings.params {
        ChannelParams::Datv(p) => {
            if p.symbol_rate.is_finite()
                && (MIN_SYMBOL_RATE..=MAX_SYMBOL_RATE).contains(&p.symbol_rate)
            {
                Ok(p)
            } else {
                Err(ChannelError::InvalidSettings(format!(
                    "DATV symbol rate must be in [{MIN_SYMBOL_RATE}, {MAX_SYMBOL_RATE}] baud, got {}",
                    p.symbol_rate
                )))
            }
        }
        ref other => Err(ChannelError::InvalidSettings(format!(
            "datv channel got {} params",
            other.type_id()
        ))),
    }
}

pub fn occupied_band(p: &DatvParams) -> (f64, f64) {
    let half = (p.symbol_rate * (1.0 + ROLL_OFF) / 2.0).min(BANDWIDTH_HZ / 2.0);
    (-half, half)
}

pub fn channel_filter(p: &DatvParams) -> Result<ChannelFilter, ChannelError> {
    let p = params(&ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Datv(*p),
        audio: sdrmm_wire::AudioProcessing::default(),
    })?;
    let (_, half) = occupied_band(&p);
    let pass = half.min(flat_bandwidth_hz(INPUT_RATE_HZ) / 2.0);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(127, pass / INPUT_RATE_HZ),
        1,
    )))
}

fn demodulator(standard: DatvStandard) -> Result<LinearDemod, ChannelError> {
    let constellation = tables::psk_rotated(4, std::f64::consts::FRAC_PI_4)
        .map_err(|error| ChannelError::InvalidSettings(format!("QPSK table: {error}")))?;
    let pulse = pulse::root_raised_cosine(SPS as f64, ROLL_OFF, PULSE_SPAN, Norm::Energy);
    let params = LinearParams::new(constellation, pulse.clone(), SPS)
        .map_err(|error| ChannelError::InvalidSettings(format!("DATV waveform: {error}")))?;
    let carrier = match standard {
        DatvStandard::DvbS => Some(
            CarrierLoop::new(PhaseDetector::MthPower { m: 4 }, 0.002).with_frequency_aid(0.005),
        ),
        DatvStandard::DvbS2 => None,
    };
    Ok(LinearDemod::new(
        &params,
        &pulse,
        LinearTiming::CONTINUOUS,
        carrier,
    ))
}

pub struct DatvChannel {
    params: DatvParams,
    acquisition: Acquisition,
    reports: Vec<Acquired>,
    resampler: FracResampler,
    demod: LinearDemod,
    decoder: DvbsDecoder,
    second: Dvbs2Decoder,
    demux: TsDemux,
    resampled: Vec<Complex<f32>>,
    symbols: Vec<Complex<f32>>,
    packets: Vec<[u8; PACKET]>,
    units: Vec<PesUnit>,
    last: Acquired,
    video_units: u64,
    audio_units: u64,
}

impl DatvChannel {
    fn rebuild(&mut self) -> Result<(), ChannelError> {
        self.acquisition = Acquisition::new(self.params.symbol_rate, INPUT_RATE_HZ);
        self.resampler = FracResampler::new(SPS as f64 * self.params.symbol_rate / INPUT_RATE_HZ);
        self.demod = demodulator(self.params.standard)?;
        self.decoder = DvbsDecoder::new(self.params.code_rate, self.params.symbol_rate);
        self.second = Dvbs2Decoder::new();
        self.demux = TsDemux::new();
        self.demux.select(self.params.program);
        self.last = Acquired::default();
        self.video_units = 0;
        self.audio_units = 0;
        Ok(())
    }

    fn system(&self) -> BroadcastSystem {
        match self.params.standard {
            DatvStandard::DvbS => BroadcastSystem::DvbS,
            DatvStandard::DvbS2 => BroadcastSystem::DvbS2,
        }
    }

    fn count(&mut self) {
        let Some(program) = self.demux.program() else {
            return;
        };
        let video: Vec<u16> = program
            .streams
            .iter()
            .filter(|stream| stream.kind.is_video())
            .map(|stream| stream.pid)
            .collect();
        let audio: Vec<u16> = program
            .streams
            .iter()
            .filter(|stream| stream.kind.is_audio())
            .map(|stream| stream.pid)
            .collect();
        for unit in &self.units {
            if video.contains(&unit.pid) {
                self.video_units += 1;
            } else if audio.contains(&unit.pid) {
                self.audio_units += 1;
            }
        }
    }

    fn services(&self) -> Vec<BroadcastService> {
        let chosen = self.demux.program().map(|program| program.number);
        self.demux
            .programs()
            .map(|program| {
                let kind = if program.streams.iter().any(|stream| stream.kind.is_video()) {
                    BroadcastServiceKind::Video
                } else if program.streams.iter().any(|stream| stream.kind.is_audio()) {
                    BroadcastServiceKind::Audio
                } else {
                    BroadcastServiceKind::Data
                };
                BroadcastService {
                    id: u32::from(program.number),
                    label: program
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Program {}", program.number)),
                    kind,
                    bitrate_kbps: None,
                    language: program
                        .streams
                        .iter()
                        .find_map(|stream| stream.language.clone()),
                    selected: chosen == Some(program.number),
                }
            })
            .collect()
    }

    fn text(&self) -> Option<String> {
        let program = self.demux.program()?;
        let mut parts: Vec<&'static str> = program
            .streams
            .iter()
            .map(|stream| stream.kind.label())
            .collect();
        parts.dedup();
        (!parts.is_empty()).then(|| parts.join(" + "))
    }

    fn coding(&self) -> Option<String> {
        match self.params.standard {
            DatvStandard::DvbS => self
                .decoder
                .running()
                .then(|| self.decoder.lock().map(|lock| lock.rate.label().to_owned()))
                .flatten(),
            DatvStandard::DvbS2 => self
                .second
                .mode()
                .map(|(modulation, rate)| format!("{} {}", modulation.label(), rate.label())),
        }
    }

    fn frames(&self) -> (u32, u32) {
        match self.params.standard {
            DatvStandard::DvbS => {
                let metrics = self.decoder.metrics();
                (metrics.packets_ok, metrics.packets_bad)
            }
            DatvStandard::DvbS2 => (
                self.second.metrics.frames_ok,
                self.second.metrics.frames_bad,
            ),
        }
    }

    fn frequency_error_hz(&self, acquired: Acquired) -> f32 {
        match self.params.standard {
            DatvStandard::DvbS => acquired.frequency_error_hz,
            DatvStandard::DvbS2 => {
                self.second.frequency_error() * self.params.symbol_rate as f32
                    / std::f32::consts::TAU
            }
        }
    }

    fn report(&mut self, acquired: Acquired, out: &mut ChannelOutputs) {
        self.last = acquired;
        let metrics = self.decoder.metrics();
        let program = self.demux.program();
        out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
            system: self.system(),
            locked: acquired.locked,
            snr_db: acquired.snr_db,
            frequency_error_hz: self.frequency_error_hz(acquired),
            symbol_rate: Some(self.params.symbol_rate),
            service_id: program.map(|program| u32::from(program.number)),
            label: program.and_then(|program| program.name.clone()),
            ensemble_label: program.and_then(|program| program.provider.clone()),
            code_rate: self.coding(),
            bit_error_rate: (self.params.standard == DatvStandard::DvbS)
                .then(|| metrics.byte_error_rate())
                .flatten(),
            frames_ok: self.frames().0,
            frames_bad: self.frames().1,
            text: self.text(),
            services: self.services(),
            ..BroadcastStatus::default()
        }));
    }
}

impl ChannelRx for DatvChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        let mut channel = Self {
            params,
            acquisition: Acquisition::new(params.symbol_rate, INPUT_RATE_HZ),
            reports: Vec::new(),
            resampler: FracResampler::new(SPS as f64 * params.symbol_rate / INPUT_RATE_HZ),
            demod: demodulator(params.standard)?,
            decoder: DvbsDecoder::new(params.code_rate, params.symbol_rate),
            second: Dvbs2Decoder::new(),
            demux: TsDemux::new(),
            resampled: Vec::new(),
            symbols: Vec::new(),
            packets: Vec::new(),
            units: Vec::new(),
            last: Acquired::default(),
            video_units: 0,
            audio_units: 0,
        };
        channel.demux.select(params.program);
        Ok(channel)
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = params(&settings)?;
        let rebuild = wanted.symbol_rate != self.params.symbol_rate
            || wanted.standard != self.params.standard
            || wanted.code_rate != self.params.code_rate;
        self.params = wanted;
        if rebuild {
            self.rebuild()?;
        } else {
            self.demux.select(wanted.program);
        }
        Ok(())
    }

    fn retuned(&mut self) {
        self.acquisition.reset();
        self.resampler.reset();
        self.demod.reset();
        self.decoder.reset();
        self.second.reset();
        self.demux.reset();
        self.video_units = 0;
        self.audio_units = 0;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let mut reports = std::mem::take(&mut self.reports);
        reports.clear();
        self.acquisition.push(iq, &mut reports);
        self.resampler.process(iq, &mut self.resampled);
        self.symbols.clear();
        let resampled = std::mem::take(&mut self.resampled);
        self.demod.process(&resampled, &mut self.symbols);
        self.resampled = resampled;
        self.packets.clear();
        let symbols = std::mem::take(&mut self.symbols);
        match self.params.standard {
            DatvStandard::DvbS => self.decoder.push(&symbols, &mut self.packets),
            DatvStandard::DvbS2 => self.second.push(&symbols, &mut self.packets),
        }
        self.symbols = symbols;
        self.units.clear();
        let mut units = std::mem::take(&mut self.units);
        for packet in &self.packets {
            self.demux.push(packet, &mut units);
        }
        self.units = units;
        self.count();
        for acquired in &reports {
            self.report(*acquired, out);
        }
        self.reports = reports;
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::DatvCodeRate;

    use super::*;
    use crate::testgen;

    fn settings(program: Option<u16>) -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            squelch_auto_db: None,
            params: ChannelParams::Datv(DatvParams {
                standard: DatvStandard::DvbS,
                symbol_rate: testgen::datv::SYMBOL_RATE,
                code_rate: DatvCodeRate::ThreeQuarters,
                program,
            }),
            audio: Default::default(),
        }
    }

    fn channel(program: Option<u16>) -> DatvChannel {
        DatvChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            settings(program),
        )
        .expect("a DATV channel at the descriptor rate")
    }

    fn drive(channel: &mut DatvChannel, iq: &[Complex<f32>]) -> Vec<BroadcastStatus> {
        let mut statuses = Vec::new();
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(16_384) {
            out.reset();
            channel.process(block, &mut out);
            for event in &out.events {
                if let DecoderEvent::Broadcast(status) = event {
                    statuses.push(status.clone());
                }
            }
        }
        statuses
    }

    #[test]
    fn a_generated_transport_stream_reaches_the_program_table() {
        let iq = testgen::datv::dvbs(3);
        let mut channel = channel(None);
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        assert!(status.locked, "{status:?}");
        assert_eq!(status.system, BroadcastSystem::DvbS);
        assert_eq!(status.code_rate.as_deref(), Some("3/4"));
        assert_eq!(status.label.as_deref(), Some(testgen::datv::PROGRAM_NAME));
        assert_eq!(
            status.ensemble_label.as_deref(),
            Some(testgen::datv::PROVIDER)
        );
        assert_eq!(status.services.len(), 1);
        assert!(status.frames_ok > 20, "{status:?}");
        assert!(channel.video_units > 0, "no video access unit arrived");
        assert!(channel.audio_units > 0, "no audio access unit arrived");
    }

    #[test]
    fn the_stream_kinds_are_named_in_the_status_text() {
        let iq = testgen::datv::dvbs(3);
        let mut channel = channel(None);
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        assert_eq!(status.text.as_deref(), Some("MPEG-2 video + MPEG-1 audio"));
        assert_eq!(status.services[0].kind, BroadcastServiceKind::Video);
    }

    fn second_generation() -> DatvChannel {
        DatvChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE_HZ,
            },
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Datv(DatvParams {
                    standard: DatvStandard::DvbS2,
                    symbol_rate: testgen::datv::SYMBOL_RATE,
                    ..DatvParams::default()
                }),
                audio: Default::default(),
            },
        )
        .expect("a DATV channel at the descriptor rate")
    }

    #[test]
    fn a_generated_second_generation_stream_reaches_the_program_table() {
        let iq = testgen::datv::dvbs2(3);
        let mut channel = second_generation();
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        assert_eq!(status.system, BroadcastSystem::DvbS2);
        assert_eq!(status.code_rate.as_deref(), Some("QPSK 3/4"));
        assert_eq!(status.label.as_deref(), Some(testgen::datv::PROGRAM_NAME));
        assert!(status.frames_ok > 3, "{status:?}");
        assert_eq!(status.frames_bad, 0, "{status:?}");
        assert!(channel.video_units > 0, "no video access unit arrived");
    }

    #[test]
    fn the_higher_order_constellations_reach_the_program_table() {
        use crate::datv::dvbs2::{frame::Modulation, ldpc::Rate};

        for (modulation, rate, label) in [
            (Modulation::Psk8, Rate::R3_5, "8PSK 3/5"),
            (Modulation::Apsk16, Rate::R3_4, "16APSK 3/4"),
            (Modulation::Apsk32, Rate::R5_6, "32APSK 5/6"),
        ] {
            let iq = testgen::datv::dvbs2_mode(3, modulation, rate, false, true);
            let mut channel = second_generation();
            let statuses = drive(&mut channel, &iq);
            let status = statuses.last().expect("a broadcast status");
            assert_eq!(status.system, BroadcastSystem::DvbS2);
            assert_eq!(status.code_rate.as_deref(), Some(label));
            assert_eq!(status.label.as_deref(), Some(testgen::datv::PROGRAM_NAME));
            assert!(status.frames_ok > 0, "{label}: {status:?}");
            assert_eq!(status.frames_bad, 0, "{label}: {status:?}");
            assert!(channel.video_units > 0, "{label} carried no video");
        }
    }

    #[test]
    fn noise_reports_neither_a_lock_nor_a_program() {
        let mut state = 0x0bad_c0deu32;
        let iq: Vec<Complex<f32>> = (0..2 * INPUT_RATE_HZ as usize)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                Complex::new(
                    (state >> 16) as f32 / 32_768.0 - 1.0,
                    (state & 0xFFFF) as f32 / 32_768.0 - 1.0,
                )
            })
            .collect();
        let mut channel = channel(None);
        let statuses = drive(&mut channel, &iq);
        assert!(statuses.iter().all(|status| !status.locked));
        assert!(statuses.iter().all(|status| status.services.is_empty()));
    }

    #[test]
    fn decoding_keeps_ahead_of_the_channel_rate() {
        let iq = testgen::datv::dvbs(2);
        let mut channel = channel(None);
        let started = std::time::Instant::now();
        drive(&mut channel, &iq);
        let elapsed = started.elapsed().as_secs_f64();
        let seconds = iq.len() as f64 / INPUT_RATE_HZ;
        assert!(
            elapsed < seconds,
            "{seconds:.2} s of DATV took {elapsed:.2} s"
        );
    }
}
