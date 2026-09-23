use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    BroadcastService, BroadcastServiceKind, ChannelDescriptor, ChannelParams, ChannelSettings,
    DecoderEvent, DecoderFamily, DvbtParams,
};

use super::frontend::Frontend;
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
    name: "DVB-T/T2".to_owned(),
    summary: "DVB-T and T2 digital TV".to_owned(),
    family: DecoderFamily::Broadcast,
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
        &design_lowpass(127, params.bandwidth.hz() / (2.0 * params.sample_rate_hz())),
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
    receiver: Frontend,
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
        let mut status = self.receiver.status(self.params.sample_rate_hz());
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
        status.service_id = selected;
        status.label = program.and_then(|p| p.name.clone());
        status.ensemble_label = program.and_then(|p| p.provider.clone());
        status.audio_frames_ok = self.media.audio_frames;
        status.audio_frames_bad = self.media.audio_errors;
        status.audio_error = self.media.audio_error.clone();
        status.video_frames_ok = self.media.video_frames;
        status.video_frames_bad = self.media.video_errors;
        status.video_error = self.media.video_error.clone();
        status.services = services;
        out.events.push(DecoderEvent::Broadcast(status));
    }
}

impl ChannelRx for DvbtChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }
    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        let params = params(&settings)?;
        check_rate(ctx, &DESCRIPTOR, params.sample_rate_hz())?;
        let mut demux = TsDemux::new();
        demux.select(params.program);
        let mut media = BroadcastMedia::new()?;
        media.enable_clock();
        Ok(Self {
            params,
            receiver: Frontend::new(params)?,
            packets: Vec::with_capacity(8192),
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
        if wanted.standard != self.params.standard
            || wanted.plp != self.params.plp
            || wanted.bandwidth != self.params.bandwidth
            || wanted.low_priority != self.params.low_priority
        {
            self.retuned();
        }
        self.receiver.apply(wanted)?;
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
        let input_rate = self.params.sample_rate_hz();
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
            blanker: Default::default(),
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
                    blanker: Default::default(),
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
    #[test]
    fn narrow_t2_channel_recovers_transport_from_a_two_megasample_radio() {
        let params = DvbtParams {
            standard: sdrmm_wire::DvbtStandard::DvbT2,
            bandwidth: sdrmm_wire::DvbtBandwidth::Mhz1_7,
            plp: Some(7),
            ..Default::default()
        };
        let mut config = settings(params.bandwidth);
        config.params = ChannelParams::Dvbt(params);
        let rate = crate::input_rate(&config.params);
        assert_eq!(rate, 131_000_000.0 / 71.0);
        let bytes = include_bytes!("../../../../../fixtures/dvbt2/rf_2k_qpsk.f32");
        let native: Vec<_> = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&[a, b, c, d, e, f, g, h]| {
                Complex::new(
                    f32::from_le_bytes([a, b, c, d]),
                    f32::from_le_bytes([e, f, g, h]),
                )
            })
            .collect();
        let iq = crate::testgen::resample(&native, rate, 2_048_000.0);
        let mut ddc = sdrmm_dsp::Ddc::new(2_048_000.0, rate, 0.0).unwrap();
        let mut filter = channel_filter(&params);
        let mut channel =
            DvbtChannel::new(ChannelCtx { input_rate: rate }, config.clone()).unwrap();
        let mut baseband = Vec::with_capacity(8192);
        let mut filtered = Vec::with_capacity(8192);
        let mut out = ChannelOutputs::default();
        for block in iq.chunks(1009) {
            baseband.clear();
            filtered.clear();
            ddc.process(block, &mut baseband);
            filter.process(&baseband, &mut filtered);
            channel.process(&filtered, &mut out);
        }
        channel.report(&mut out);
        let status = out
            .events
            .iter()
            .rev()
            .find_map(|event| {
                if let DecoderEvent::Broadcast(status) = event {
                    Some(status)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(status.system, sdrmm_wire::BroadcastSystem::DvbT2);
        assert_eq!(status.frames_ok, 4, "{status:?}");
        assert_eq!(status.frames_bad, 0, "{status:?}");
        let mut changed = params;
        changed.plp = Some(9);
        config.params = ChannelParams::Dvbt(changed);
        channel.apply(config).unwrap();
        assert!(!channel.receiver.locked());
    }

    #[test]
    fn t2_iq_discovers_program_and_delivers_audio_and_video() {
        let params = DvbtParams {
            standard: sdrmm_wire::DvbtStandard::DvbT2,
            ..Default::default()
        };
        let mut config = settings(params.bandwidth);
        config.params = ChannelParams::Dvbt(params);
        let rate = params.sample_rate_hz();
        let bytes = include_bytes!("../../../../../fixtures/dvbt2/rf_32k_media.f32");
        let iq: Vec<_> = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&[a, b, c, d, e, f, g, h]| {
                Complex::new(
                    f32::from_le_bytes([a, b, c, d]),
                    f32::from_le_bytes([e, f, g, h]),
                )
            })
            .collect();
        let mut channel = DvbtChannel::new(ChannelCtx { input_rate: rate }, config).unwrap();
        let mut out = ChannelOutputs::default();
        for block in iq[..iq.len() - 8192 + 32].chunks(1009) {
            channel.process(block, &mut out);
        }
        assert_eq!(channel.demux.program().map(|p| p.number), Some(1));
        assert!(channel.receiver.locked());
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            channel.media.advance((rate / 50.0) as usize, rate);
            channel.media.drain(&mut out);
            if channel.media.audio_frames > 0 && channel.media.video_frames > 0 {
                break;
            }
        }
        assert_eq!(
            channel.media.audio_errors, 0,
            "{:?}",
            channel.media.audio_error
        );
        assert_eq!(
            channel.media.video_errors, 0,
            "{:?}",
            channel.media.video_error
        );
        assert!(channel.media.audio_frames > 0);
        assert!(channel.media.video_frames > 0);
        assert!(out.audio_pcm.iter().any(|&sample| sample.abs() > 0.01));
    }
}
