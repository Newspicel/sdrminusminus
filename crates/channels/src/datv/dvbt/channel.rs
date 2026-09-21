use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, BroadcastStatus, BroadcastSystem, ChannelDescriptor,
    ChannelParams, ChannelSettings, DecoderEvent, DvbtParams,
};

use super::receiver::Receiver;
use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    broadcast_media::BroadcastMedia,
    check_rate,
    datv::{
        channel::media_kind,
        dvbs::PACKET,
        ts::{PesUnit, StreamKind, TsDemux},
    },
};

pub const INPUT_RATE: f64 = 64_000_000.0 / 7.0;
static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dvbt".to_owned(),
    name: "DVB-T".to_owned(),
    bandwidth_hz: 8_000_000.0,
    input_rate_hz: INPUT_RATE,
    has_audio: true,
    has_video: true,
    decoder_kind: Some("broadcast".to_owned()),
    ..ChannelDescriptor::default()
});

pub fn occupied_band(params: &DvbtParams) -> (f64, f64) {
    let half = params.bandwidth.hz() / 2.0;
    (-half, half)
}

pub fn channel_filter(params: &DvbtParams) -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(
            127,
            params.bandwidth.hz() / (2.0 * params.bandwidth.sample_rate_hz()),
        ),
        1,
    ))
}

fn params(settings: &ChannelSettings) -> Result<DvbtParams, ChannelError> {
    match settings.params {
        ChannelParams::Dvbt(params) => Ok(params),
        ref other => Err(ChannelError::InvalidSettings(format!(
            "DVB-T received {} settings",
            other.type_id()
        ))),
    }
}

#[derive(PartialEq, Eq)]
struct Selection {
    program: u16,
    audio: Option<(u16, StreamKind)>,
    video: Option<(u16, StreamKind)>,
}

pub struct DvbtChannel {
    params: DvbtParams,
    receiver: Receiver,
    packets: Vec<[u8; PACKET]>,
    units: Vec<PesUnit>,
    demux: TsDemux,
    media: BroadcastMedia,
    selection: Option<Selection>,
    report_samples: usize,
    was_locked: bool,
}

impl DvbtChannel {
    fn play(&mut self, out: &mut ChannelOutputs) {
        let selected = self.demux.program().map(|program| Selection {
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
        if selected != self.selection {
            self.media.reset();
            self.selection = selected;
        }
        if let Some(selection) = &self.selection {
            for unit in &self.units {
                for (pid, kind) in [selection.audio, selection.video].into_iter().flatten() {
                    if unit.pid == pid
                        && let Some(kind) = media_kind(kind)
                    {
                        self.media.push(kind, &unit.payload, unit.pts, None);
                    }
                }
            }
        }
        self.media.drain(out);
    }

    fn report(&self, out: &mut ChannelOutputs) {
        let program = self.demux.program();
        let metrics = self.receiver.metrics();
        let parameters = self.receiver.parameters;
        let selected = program.map(|p| u32::from(p.number));
        let services = self
            .demux
            .programs()
            .map(|p| BroadcastService {
                id: u32::from(p.number),
                label: p
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Program {}", p.number)),
                kind: if p.streams.iter().any(|s| s.kind.is_video()) {
                    BroadcastServiceKind::Video
                } else if p.streams.iter().any(|s| s.kind.is_audio()) {
                    BroadcastServiceKind::Audio
                } else {
                    BroadcastServiceKind::Data
                },
                selected: selected == Some(u32::from(p.number)),
                bitrate_kbps: None,
                language: p.streams.iter().find_map(|s| s.language.clone()),
            })
            .collect();
        out.events.push(DecoderEvent::Broadcast(BroadcastStatus {
            system: BroadcastSystem::DvbT,
            locked: self.receiver.locked(),
            snr_db: self.receiver.snr,
            frequency_error_hz: self.receiver.frequency
                * self.params.bandwidth.sample_rate_hz() as f32
                / std::f32::consts::TAU,
            service_id: selected,
            label: program.and_then(|p| p.name.clone()),
            ensemble_label: program.and_then(|p| p.provider.clone()),
            code_rate: parameters.map(|p| {
                format!(
                    "{}K {} {} 1/{}",
                    p.fft / 1024,
                    match p.bits {
                        2 => "QPSK",
                        4 => "16-QAM",
                        _ => "64-QAM",
                    },
                    if p.hierarchical && self.params.low_priority {
                        p.low_rate
                    } else {
                        p.high_rate
                    }
                    .label(),
                    p.fft / p.guard
                )
            }),
            frames_ok: metrics.packets_ok,
            frames_bad: metrics
                .packets_bad
                .saturating_add(self.receiver.bad_symbols),
            bit_error_rate: metrics.byte_error_rate(),
            audio_frames_ok: self.media.audio_frames,
            audio_frames_bad: self.media.audio_errors,
            audio_error: self.media.audio_error.clone(),
            video_frames_ok: self.media.video_frames,
            video_frames_bad: self.media.video_errors,
            video_error: self.media.video_error.clone(),
            services,
            ..BroadcastStatus::default()
        }));
    }
}

impl ChannelRx for DvbtChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }
    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        let params = params(&settings)?;
        check_rate(ctx, &DESCRIPTOR, params.bandwidth.sample_rate_hz())?;
        let mut demux = TsDemux::new();
        demux.select(params.program);
        let mut media = BroadcastMedia::new()?;
        media.enable_clock();
        Ok(Self {
            params,
            receiver: Receiver::new(params.low_priority),
            packets: Vec::with_capacity(256),
            units: Vec::with_capacity(32),
            demux,
            media,
            selection: None,
            report_samples: 0,
            was_locked: false,
        })
    }
    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let wanted = params(&settings)?;
        if wanted.bandwidth != self.params.bandwidth
            || wanted.low_priority != self.params.low_priority
        {
            self.retuned();
            self.receiver.low_priority = wanted.low_priority;
        }
        self.demux.select(wanted.program);
        self.params = wanted;
        Ok(())
    }
    fn retuned(&mut self) {
        self.receiver.reset();
        self.demux.reset();
        self.media.reset();
        self.selection = None;
        self.report_samples = 0;
        self.was_locked = false;
    }
    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let input_rate = self.params.bandwidth.sample_rate_hz();
        self.media.advance(iq.len(), input_rate);
        self.packets.clear();
        self.receiver.push(iq, &mut self.packets);
        let locked = self.receiver.locked();
        if self.was_locked && !locked {
            self.demux.reset();
            self.media.reset();
            self.selection = None;
        }
        self.was_locked = locked;
        self.units.clear();
        for packet in &self.packets {
            self.demux.push(packet, &mut self.units);
        }
        self.play(out);
        self.report_samples += iq.len();
        if self.report_samples >= (input_rate / 4.0) as usize {
            self.report_samples %= (input_rate / 4.0) as usize;
            self.report(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(bandwidth: sdrmm_wire::DvbtBandwidth) -> ChannelSettings {
        ChannelSettings {
            frequency_hz: 0.0,
            squelch: sdrmm_wire::Squelch::Off,
            params: ChannelParams::Dvbt(DvbtParams {
                bandwidth,
                ..Default::default()
            }),
            audio: Default::default(),
        }
    }

    #[test]
    fn narrow_channel_decodes_from_a_two_megasample_radio() {
        let bandwidth = sdrmm_wire::DvbtBandwidth::Mhz1_7;
        let channel_settings = settings(bandwidth);
        let input_rate = crate::input_rate(&channel_settings.params);
        assert_eq!(input_rate, bandwidth.sample_rate_hz());
        assert_eq!(
            crate::occupied_band(&channel_settings.params),
            (-850_000.0, 850_000.0)
        );
        let mut native = crate::testgen::dvbt::waveform(crate::testgen::dvbt::defaults(), 180);
        crate::testgen::shift(&mut native, 600.0, input_rate);
        let iq = crate::testgen::resample(&native, input_rate, 2_048_000.0);
        let mut ddc = sdrmm_dsp::Ddc::new(2_048_000.0, input_rate, 0.0).unwrap();
        let mut filter = crate::channel_filter(&channel_settings.params).unwrap();
        let mut receiver = DvbtChannel::new(ChannelCtx { input_rate }, channel_settings).unwrap();
        let mut baseband = Vec::new();
        let mut filtered = Vec::new();
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(1009) {
            ddc.process(block, &mut baseband);
            filter.process(&baseband, &mut filtered);
            out.reset();
            receiver.process(&filtered, &mut out);
        }
        assert!(receiver.receiver.locked());
        assert_eq!(
            receiver.demux.program().and_then(|p| p.name.as_deref()),
            Some("Rust TV")
        );
        out.reset();
        receiver.report(&mut out);
        let DecoderEvent::Broadcast(status) = &out.events[0] else {
            panic!("broadcast status");
        };
        assert!((status.frequency_error_hz - 600.0).abs() < 30.0);
        assert!(
            DvbtChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE
                },
                settings(bandwidth)
            )
            .is_err()
        );
    }

    #[test]
    fn changing_bandwidth_discards_decoder_and_media_state() {
        let bandwidth = sdrmm_wire::DvbtBandwidth::Mhz1_7;
        let mut receiver = DvbtChannel::new(
            ChannelCtx {
                input_rate: INPUT_RATE,
            },
            settings(sdrmm_wire::DvbtBandwidth::Mhz8),
        )
        .unwrap();
        let iq = crate::testgen::dvbt::waveform(crate::testgen::dvbt::defaults(), 180);
        let mut out = ChannelOutputs::default();
        receiver.process(&iq, &mut out);
        assert!(receiver.receiver.locked());
        receiver.apply(settings(bandwidth)).unwrap();
        assert!(!receiver.receiver.locked());
        assert!(receiver.selection.is_none());
        assert_eq!(receiver.report_samples, 0);
        out.reset();
        receiver.process(&iq, &mut out);
        assert!(receiver.receiver.locked());
    }

    #[test]
    fn every_bandwidth_discovers_services_at_its_native_clock() {
        let iq = crate::testgen::dvbt::waveform(crate::testgen::dvbt::defaults(), 180);
        for bandwidth in [
            sdrmm_wire::DvbtBandwidth::Mhz1_7,
            sdrmm_wire::DvbtBandwidth::Mhz6,
            sdrmm_wire::DvbtBandwidth::Mhz7,
            sdrmm_wire::DvbtBandwidth::Mhz8,
        ] {
            let params = DvbtParams {
                bandwidth,
                ..Default::default()
            };
            let mut channel = DvbtChannel::new(
                ChannelCtx {
                    input_rate: bandwidth.sample_rate_hz(),
                },
                ChannelSettings {
                    frequency_hz: 0.0,
                    squelch: sdrmm_wire::Squelch::Off,
                    params: ChannelParams::Dvbt(params),
                    audio: Default::default(),
                },
            )
            .unwrap();
            let mut out = ChannelOutputs::default();
            for block in iq.chunks(16384) {
                out.reset();
                channel.process(block, &mut out);
            }
            assert!(channel.receiver.locked(), "{bandwidth:?}");
            assert_eq!(
                channel.demux.program().and_then(|p| p.name.as_deref()),
                Some("Rust TV")
            );
        }
    }
}
