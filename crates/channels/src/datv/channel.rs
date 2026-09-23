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
    ChannelParams, ChannelSettings, DatvParams, DatvRollOff, DatvStandard, DecoderEvent,
    DecoderFamily, MAX_DATV_SYMBOL_RATE, MIN_DATV_SYMBOL_RATE,
};

use super::{
    acquire::{Acquired, Acquisition},
    dvbs::{DvbsDecoder, PACKET},
    dvbs2::{
        bb::StreamKind,
        gse::protocol_name,
        receiver::{Dvbs2Decoder, Dvbs2Output},
    },
    ts::{PesUnit, StreamKind as ElementaryKind, TsDemux},
};
use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    broadcast_media::{BroadcastMedia, Kind as MediaKind},
    check_rate,
};

const MIN_INPUT_RATE_HZ: f64 = 2_000_000.0;
const SPS: usize = 4;
const PULSE_SPAN: usize = 8;
const MAX_PROTOCOLS: usize = 8;

pub fn occupied_hz(p: &DatvParams) -> f64 {
    (p.symbol_rate * (1.0 + p.roll_off.factor())).round()
}

pub fn input_rate_hz(p: &DatvParams) -> f64 {
    let mut rate = MIN_INPUT_RATE_HZ;
    while flat_bandwidth_hz(rate) < occupied_hz(p) {
        rate *= 2.0;
    }
    rate
}

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "datv".to_owned(),
    name: "DATV (DVB-S / S2)".to_owned(),
    summary: "Digital amateur TV over DVB-S and S2".to_owned(),
    family: DecoderFamily::Broadcast,
    bandwidth_hz: occupied_hz(&DatvParams::default()),
    input_rate_hz: input_rate_hz(&DatvParams::default()),
    has_audio: true,
    has_video: true,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

fn params(settings: &ChannelSettings) -> Result<DatvParams, ChannelError> {
    match settings.params {
        ChannelParams::Datv(p) => {
            if p.symbol_rate.is_finite()
                && (MIN_DATV_SYMBOL_RATE..=MAX_DATV_SYMBOL_RATE).contains(&p.symbol_rate)
            {
                Ok(p)
            } else {
                Err(ChannelError::InvalidSettings(format!(
                    "DATV symbol rate must be in [{MIN_DATV_SYMBOL_RATE}, {MAX_DATV_SYMBOL_RATE}] baud, got {}",
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
    let half = occupied_hz(p) / 2.0;
    (-half, half)
}

pub fn channel_filter(p: &DatvParams) -> Result<ChannelFilter, ChannelError> {
    let p = params(&ChannelSettings {
        frequency_hz: 0.0,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Datv(*p),
        audio: sdrmm_wire::AudioProcessing::default(),
    })?;
    let (_, half) = occupied_band(&p);
    let rate = input_rate_hz(&p);
    let pass = half.min(flat_bandwidth_hz(rate) / 2.0);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(127, pass / rate),
        1,
    )))
}

fn demodulator(standard: DatvStandard, roll_off: DatvRollOff) -> Result<LinearDemod, ChannelError> {
    let constellation = tables::psk_rotated(4, std::f64::consts::FRAC_PI_4)
        .map_err(|error| ChannelError::InvalidSettings(format!("QPSK table: {error}")))?;
    let pulse = pulse::root_raised_cosine(SPS as f64, roll_off.factor(), PULSE_SPAN, Norm::Energy);
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct MediaSelection {
    program: u16,
    audio: Option<(u16, ElementaryKind)>,
    video: Option<(u16, ElementaryKind)>,
}

pub(super) fn media_kind(kind: ElementaryKind) -> Option<MediaKind> {
    match kind {
        ElementaryKind::Mpeg1Audio | ElementaryKind::Mpeg2Audio => Some(MediaKind::Mp2),
        ElementaryKind::Eac3Audio => Some(MediaKind::Eac3),
        ElementaryKind::AacAudio => Some(MediaKind::Aac),
        ElementaryKind::LatmAudio => Some(MediaKind::Latm),
        ElementaryKind::Ac3Audio => Some(MediaKind::Ac3),
        ElementaryKind::Mpeg2Video => Some(MediaKind::Mpeg2),
        ElementaryKind::H264Video => Some(MediaKind::H264),
        ElementaryKind::H265Video => Some(MediaKind::H265),
        _ => None,
    }
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
    second_out: Dvbs2Output,
    protocols: Vec<u16>,
    units: Vec<PesUnit>,
    last: Acquired,
    video_units: u64,
    audio_units: u64,
    media: BroadcastMedia,
    media_selection: Option<MediaSelection>,
}

impl DatvChannel {
    fn rebuild(&mut self) -> Result<(), ChannelError> {
        let rate = input_rate_hz(&self.params);
        self.acquisition = Acquisition::new(self.params.symbol_rate, rate);
        self.resampler = FracResampler::new(SPS as f64 * self.params.symbol_rate / rate);
        self.demod = demodulator(self.params.standard, self.params.roll_off)?;
        self.decoder = DvbsDecoder::new(self.params.code_rate, self.params.symbol_rate);
        self.second = Dvbs2Decoder::new();
        self.demux = TsDemux::new();
        self.demux.select(self.params.program);
        self.second.select(self.params.input_stream);
        self.second.superframes(self.params.superframes);
        self.protocols.clear();
        self.last = Acquired::default();
        self.video_units = 0;
        self.audio_units = 0;
        self.media.reset();
        self.media_selection = None;
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

    fn encapsulated(&self) -> Option<StreamKind> {
        self.second
            .stream
            .filter(|kind| self.params.standard == DatvStandard::DvbS2 && kind.is_encapsulated())
    }

    fn read_audio(&mut self, out: &mut ChannelOutputs) {
        let selection = self.demux.program().map(|program| MediaSelection {
            program: program.number,
            audio: program
                .streams
                .iter()
                .find(|s| s.kind.is_audio())
                .map(|s| (s.pid, s.kind)),
            video: program
                .streams
                .iter()
                .find(|s| s.kind.is_video())
                .map(|s| (s.pid, s.kind)),
        });
        if selection != self.media_selection {
            self.media.reset();
            self.media_selection = selection;
        }
        if let Some(selection) = &self.media_selection {
            for unit in &self.units {
                if let Some((pid, kind)) = selection.audio
                    && unit.pid == pid
                    && let Some(kind) = media_kind(kind)
                {
                    self.media.push(kind, &unit.payload, unit.pts, None);
                }
                if let Some((pid, kind)) = selection.video
                    && unit.pid == pid
                    && let Some(kind) = media_kind(kind)
                {
                    self.media.push(kind, &unit.payload, unit.pts, None);
                }
            }
        }
        self.media.drain(out);
    }

    fn streams(&self) -> Vec<BroadcastService> {
        self.second
            .streams()
            .into_iter()
            .map(|(isi, _)| BroadcastService {
                id: u32::from(isi),
                label: format!("Stream {isi}"),
                kind: BroadcastServiceKind::Data,
                bitrate_kbps: None,
                language: None,
                selected: self.params.input_stream.is_none_or(|chosen| chosen == isi),
            })
            .collect()
    }

    fn services(&self) -> Vec<BroadcastService> {
        if self.encapsulated().is_some() {
            return self.streams();
        }
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
        if let Some(kind) = self.encapsulated() {
            let parts: Vec<&'static str> = self
                .protocols
                .iter()
                .map(|&protocol| protocol_name(protocol))
                .collect();
            return Some(if parts.is_empty() {
                kind.label().to_owned()
            } else {
                format!("{} + {}", kind.label(), parts.join(" + "))
            });
        }
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
            DatvStandard::DvbS2 => self.second.very_low_mode().map_or_else(
                || {
                    self.second.mode().map(|(modulation, rate)| {
                        format!("{} {}", modulation.label(), rate.label())
                    })
                },
                |mode| Some(format!("VL-SNR {}", mode.label)),
            ),
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
                self.second
                    .metrics
                    .frames_bad
                    .saturating_add(self.second.metrics.transport_errors),
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
            audio_frames_ok: self.media.audio_frames,
            audio_frames_bad: self.media.audio_errors,
            audio_error: self.media.audio_error.clone(),
            video_frames_ok: self.media.video_frames,
            video_frames_bad: self.media.video_errors,
            video_error: self.media.video_error.clone(),
            locked: match self.params.standard {
                DatvStandard::DvbS => acquired.locked,
                DatvStandard::DvbS2 => acquired.locked || self.second.locked(),
            },
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
            frames_bad: self
                .frames()
                .1
                .saturating_add(self.demux.dropped)
                .saturating_add(self.demux.discontinuities),
            data_error: self
                .second
                .superframe_format()
                .filter(|format| *format > 1)
                .map(|format| format!("Unsupported superframe format {format}"))
                .or_else(|| {
                    (self.demux.scrambled > 0).then(|| "Selected programme is scrambled".to_owned())
                }),
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
        let params = params(&settings)?;
        let rate = input_rate_hz(&params);
        check_rate(ctx, &DESCRIPTOR, rate)?;
        let mut channel = Self {
            params,
            acquisition: Acquisition::new(params.symbol_rate, rate),
            reports: Vec::new(),
            resampler: FracResampler::new(SPS as f64 * params.symbol_rate / rate),
            demod: demodulator(params.standard, params.roll_off)?,
            decoder: DvbsDecoder::new(params.code_rate, params.symbol_rate),
            second: Dvbs2Decoder::new(),
            demux: TsDemux::new(),
            resampled: Vec::new(),
            symbols: Vec::new(),
            packets: Vec::new(),
            second_out: Dvbs2Output::default(),
            protocols: Vec::new(),
            units: Vec::new(),
            last: Acquired::default(),
            video_units: 0,
            audio_units: 0,
            media: BroadcastMedia::new()?,
            media_selection: None,
        };
        channel.media.enable_clock();
        channel.demux.select(params.program);
        channel.second.select(params.input_stream);
        channel.second.superframes(params.superframes);
        Ok(channel)
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = params(&settings)?;
        let rebuild = wanted.symbol_rate != self.params.symbol_rate
            || wanted.standard != self.params.standard
            || wanted.code_rate != self.params.code_rate
            || wanted.roll_off != self.params.roll_off;
        self.params = wanted;
        if rebuild {
            self.rebuild()?;
        } else {
            self.demux.select(wanted.program);
            self.second.select(wanted.input_stream);
            self.second.superframes(wanted.superframes);
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
        self.protocols.clear();
        self.video_units = 0;
        self.audio_units = 0;
        self.media.reset();
        self.media_selection = None;
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.media.advance(iq.len(), input_rate_hz(&self.params));
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
            DatvStandard::DvbS2 => {
                let mut second = std::mem::take(&mut self.second_out);
                second.clear();
                self.second.push(&symbols, &mut second);
                self.packets.extend_from_slice(&second.packets);
                for (index, pdu) in second.pdus.iter().enumerate() {
                    out.events
                        .push(DecoderEvent::BroadcastData(sdrmm_wire::BroadcastData {
                            service_id: self.params.input_stream.map(u32::from),
                            protocol: Some(pdu.protocol),
                            label: pdu.label.clone(),
                            name: format!("GSE-{}-{index}.bin", self.second.metrics.frames_ok),
                            media_type: "application/octet-stream".to_owned(),
                            bytes: pdu.data.clone(),
                        }));
                    if !self.protocols.contains(&pdu.protocol)
                        && self.protocols.len() < MAX_PROTOCOLS
                    {
                        self.protocols.push(pdu.protocol);
                        self.protocols.sort_unstable();
                    }
                }
                self.second_out = second;
            }
        }
        self.symbols = symbols;
        self.units.clear();
        let mut units = std::mem::take(&mut self.units);
        for packet in &self.packets {
            self.demux.push(packet, &mut units);
        }
        self.units = units;
        self.count();
        self.read_audio(out);
        for acquired in &reports {
            self.report(*acquired, out);
        }
        self.reports = reports;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{testgen, testutil::realtime_budget};

    fn settings_of(params: DatvParams) -> ChannelSettings {
        ChannelSettings {
            frequency_hz: 0.0,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Datv(params),
            audio: Default::default(),
        }
    }

    fn open(params: DatvParams) -> DatvChannel {
        DatvChannel::new(
            ChannelCtx {
                input_rate: input_rate_hz(&params),
            },
            settings_of(params),
        )
        .expect("a DATV channel at its own input rate")
    }

    fn channel(program: Option<u16>) -> DatvChannel {
        open(DatvParams {
            program,
            ..testgen::datv::params()
        })
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
    fn the_channel_rate_follows_the_symbol_rate() {
        let narrow = DatvParams {
            symbol_rate: 333_000.0,
            ..DatvParams::default()
        };
        let wide = DatvParams {
            symbol_rate: 2_330_000.0,
            roll_off: DatvRollOff::Pct25,
            ..DatvParams::default()
        };
        assert_eq!(input_rate_hz(&narrow), 2_000_000.0);
        assert_eq!(input_rate_hz(&wide), 4_000_000.0);
        assert_eq!(
            input_rate_hz(&DatvParams {
                symbol_rate: MAX_DATV_SYMBOL_RATE,
                ..DatvParams::default()
            }),
            8_000_000.0
        );
    }

    #[test]
    fn an_iss_wide_carrier_locks_and_decodes() {
        let params = DatvParams {
            symbol_rate: 2_000_000.0,
            ..testgen::datv::params()
        };
        let iq = testgen::datv::dvbs_with(1, &params);
        let mut channel = open(params);
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        assert!(status.locked, "{status:?}");
        assert!(status.frames_ok > 0, "{status:?}");
        assert_eq!(status.label.as_deref(), Some(testgen::datv::PROGRAM_NAME));
    }

    const DOPPLER_HZ: f64 = 55_000.0;
    const DOPPLER_DRIFT_HZ_PER_S: f64 = -1_000.0;
    const ISS_ES_N0_DB: f32 = 4.0;

    fn doppler(iq: &mut [Complex<f32>], rate: f64, offset_hz: f64, drift_hz_per_s: f64) {
        for (index, sample) in iq.iter_mut().enumerate() {
            let t = index as f64 / rate;
            let phase = std::f64::consts::TAU * (offset_hz * t + 0.5 * drift_hz_per_s * t * t);
            *sample *= Complex::from_polar(1.0, phase as f32);
        }
    }

    #[test]
    fn an_iss_second_generation_carrier_rides_its_doppler() {
        use crate::datv::dvbs2::{frame::Modulation, ldpc::Rate};
        let params = DatvParams {
            standard: DatvStandard::DvbS2,
            symbol_rate: 2_000_000.0,
            ..DatvParams::default()
        };
        let rate = input_rate_hz(&params);
        let mut iq = testgen::datv::dvbs2_with(2, &params, Modulation::Qpsk, Rate::R1_2);
        doppler(&mut iq, rate, DOPPLER_HZ, DOPPLER_DRIFT_HZ_PER_S);
        let power = iq.iter().map(|value| value.norm_sqr()).sum::<f32>() / iq.len() as f32;
        let noise = power * (rate / params.symbol_rate) as f32 / 10f32.powf(ISS_ES_N0_DB / 10.0);
        testgen::add_noise(&mut iq, 7, (1.5 * noise).sqrt());
        let mut channel = open(params);
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        assert!(status.locked, "{status:?}");
        assert!(status.frames_ok > 100, "{status:?}");
        assert_eq!(status.label.as_deref(), Some(testgen::datv::PROGRAM_NAME));
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
        let mut out = ChannelOutputs::default();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while channel.media.video_frames == 0 && std::time::Instant::now() < until {
            channel
                .media
                .advance(16384, input_rate_hz(&testgen::datv::params()));
            channel.media.drain(&mut out);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            channel.media.video_frames > 0,
            "{:?}",
            channel.media.video_error
        );
        assert!(
            channel.media.audio_frames > 0,
            "{:?}",
            channel.media.audio_error
        );
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
        open(DatvParams {
            standard: DatvStandard::DvbS2,
            symbol_rate: testgen::datv::SYMBOL_RATE,
            ..DatvParams::default()
        })
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
            assert!(status.frames_ok > 0, "{label}: {status:?}");
            assert_eq!(status.frames_bad, 0, "{label}: {status:?}");
            assert_eq!(
                status.label.as_deref(),
                Some(testgen::datv::PROGRAM_NAME),
                "{status:?}"
            );
            assert!(channel.video_units > 0, "{label} carried no video");
        }
    }

    #[test]
    fn s2x_high_order_iq_reaches_programmes_at_a_bounded_cost() {
        use crate::datv::dvbs2::{frame::Modulation, ldpc::Rate};
        for (modulation, rate) in [
            (Modulation::Apsk8, Rate::R100_180),
            (Modulation::Apsk64, Rate::R128_180),
            (Modulation::Apsk128, Rate::R135_180),
            (Modulation::Apsk256, Rate::R116_180),
        ] {
            let iq = testgen::datv::dvbs2_mode(2, modulation, rate, false, true);
            let mut channel = second_generation();
            let started = std::time::Instant::now();
            let statuses = drive(&mut channel, &iq);
            let elapsed = started.elapsed().as_secs_f64();
            let status = statuses.last().expect("broadcast status");
            assert_eq!(
                status.label.as_deref(),
                Some(testgen::datv::PROGRAM_NAME),
                "{modulation:?}: {status:?}"
            );
            assert!(status.frames_ok > 0, "{modulation:?}: {status:?}");
            // Four times the signal's own duration, not once: these are the heaviest modcods
            // the standard defines, and a hosted runner's speed moves by nearly two to one
            // between runs. A tighter bound reports which machine picked up the job; this one
            // still catches a decoder that has halved in speed.
            assert!(
                elapsed < realtime_budget(8.0),
                "{modulation:?}: {elapsed:.3}s for two seconds of IQ"
            );
        }
    }

    #[test]
    fn an_encapsulated_stream_is_named_and_its_input_streams_listed() {
        let iq = testgen::datv::dvbs2_generic(3, &[4, 11]);
        let mut channel = second_generation();
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        assert_eq!(status.system, BroadcastSystem::DvbS2);
        assert_eq!(status.text.as_deref(), Some("GSE + IPv4"));
        assert_eq!(status.code_rate.as_deref(), Some("16APSK 3/4"));
        assert!(status.frames_ok > 4, "{status:?}");
        let ids: Vec<u32> = status.services.iter().map(|service| service.id).collect();
        assert_eq!(ids, vec![4, 11]);
        assert!(status.services.iter().all(|service| service.selected));
        assert!(
            status
                .services
                .iter()
                .all(|service| service.kind == BroadcastServiceKind::Data)
        );
    }

    #[test]
    fn only_the_chosen_input_stream_is_read() {
        let iq = testgen::datv::dvbs2_generic(3, &[4, 11]);
        let mut channel = open(DatvParams {
            standard: DatvStandard::DvbS2,
            symbol_rate: testgen::datv::SYMBOL_RATE,
            input_stream: Some(11),
            ..DatvParams::default()
        });
        let statuses = drive(&mut channel, &iq);
        let status = statuses.last().expect("a broadcast status");
        let chosen: Vec<u32> = status
            .services
            .iter()
            .filter(|service| service.selected)
            .map(|service| service.id)
            .collect();
        assert_eq!(chosen, vec![11]);
        assert!(channel.second.metrics.frames_skipped > 0);
    }

    #[test]
    fn a_very_low_signal_stream_reaches_the_program_table() {
        for (header, label) in [(9u8, "VL-SNR BPSK 1/5"), (0, "VL-SNR QPSK 2/9")] {
            let iq = testgen::datv::dvbs2_very_low(8, header);
            let mut channel = second_generation();
            let statuses = drive(&mut channel, &iq);
            let status = statuses.last().expect("a broadcast status");
            assert_eq!(status.system, BroadcastSystem::DvbS2);
            assert_eq!(status.code_rate.as_deref(), Some(label));
            assert!(status.frames_ok > 0, "{label}: {status:?}");
            assert_eq!(status.frames_bad, 0, "{label}: {status:?}");
            assert_eq!(
                status.label.as_deref(),
                Some(testgen::datv::PROGRAM_NAME),
                "{status:?}"
            );
            assert!(channel.video_units > 0, "{label} carried no video");
        }
    }

    #[test]
    fn noise_reports_neither_a_lock_nor_a_program() {
        let mut state = 0x0bad_c0deu32;
        let iq: Vec<Complex<f32>> = (0..2 * input_rate_hz(&testgen::datv::params()) as usize)
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
    fn the_higher_order_constellations_keep_ahead_of_the_channel_rate() {
        use crate::datv::dvbs2::{frame::Modulation, ldpc::Rate};

        let iq = testgen::datv::dvbs2_mode(2, Modulation::Apsk32, Rate::R5_6, false, true);
        let mut channel = second_generation();
        let started = std::time::Instant::now();
        let statuses = drive(&mut channel, &iq);
        let elapsed = started.elapsed().as_secs_f64();
        let seconds = iq.len() as f64 / input_rate_hz(&testgen::datv::params());
        assert!(
            statuses.last().is_some_and(|status| status.frames_ok > 0),
            "no 32APSK frame decoded, so the timing proves nothing"
        );
        assert!(
            elapsed < realtime_budget(seconds),
            "{seconds:.2} s of 32APSK took {elapsed:.2} s"
        );
    }

    #[test]
    fn a_very_low_signal_frame_keeps_ahead_of_the_channel_rate() {
        let iq = testgen::datv::dvbs2_very_low(2, 0);
        let mut channel = second_generation();
        let started = std::time::Instant::now();
        let statuses = drive(&mut channel, &iq);
        let elapsed = started.elapsed().as_secs_f64();
        let seconds = iq.len() as f64 / input_rate_hz(&testgen::datv::params());
        assert!(
            statuses.last().is_some_and(|status| status.frames_ok > 0),
            "no VL-SNR frame decoded, so the timing proves nothing"
        );
        assert!(
            elapsed < realtime_budget(seconds),
            "{seconds:.2} s of VL-SNR took {elapsed:.2} s"
        );
    }

    #[test]
    fn decoding_keeps_ahead_of_the_channel_rate() {
        let iq = testgen::datv::dvbs(2);
        let mut channel = channel(None);
        let started = std::time::Instant::now();
        drive(&mut channel, &iq);
        let elapsed = started.elapsed().as_secs_f64();
        let seconds = iq.len() as f64 / input_rate_hz(&testgen::datv::params());
        assert!(
            elapsed < realtime_budget(seconds),
            "{seconds:.2} s of DATV took {elapsed:.2} s"
        );
    }
}
