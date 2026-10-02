use std::time::{Duration, Instant};

use sdrmm_wire::{ChannelParams, DrmMode};

use super::*;
use crate::{
    drm::coding::Qam,
    synth::{
        self,
        drm::{Config, defaults, multipath, music, talk},
    },
    testutil::{at_snr, realtime_budget},
};

fn settings(mode: DrmMode, bandwidth_hz: f64, service: Option<u8>) -> ChannelSettings {
    ChannelSettings {
        frequency_hz: 0.0,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Drm(DrmParams {
            mode,
            bandwidth_hz,
            service,
        }),
        blanker: Default::default(),
    }
}

fn channel(mode: DrmMode, service: Option<u8>) -> DrmChannel {
    let bandwidth = if mode == DrmMode::Drm30 {
        20_000.0
    } else {
        100_000.0
    };
    DrmChannel::new(
        ChannelCtx {
            input_rate: INPUT_RATE_HZ,
        },
        settings(mode, bandwidth, service),
    )
    .expect("a DRM channel")
}

fn status(out: &ChannelOutputs) -> &BroadcastStatus {
    out.events
        .iter()
        .rev()
        .find_map(|event| match event {
            DecoderEvent::Broadcast(status) => Some(status),
            _ => None,
        })
        .expect("a broadcast status")
}

fn run(channel: &mut DrmChannel, iq: &[Complex<f32>]) -> ChannelOutputs {
    let mut out = ChannelOutputs::default();
    for block in iq.chunks(9_973) {
        channel.process(block, &mut out);
    }
    let until = Instant::now() + Duration::from_secs(3);
    while channel.media.audio_frames < 20 && Instant::now() < until {
        channel.collect_audio(&mut out);
        std::thread::sleep(Duration::from_millis(2));
    }
    channel.report(&mut out);
    out
}

fn two_services(mode: Robustness) -> Config {
    Config {
        occupancy: 5,
        msc: Qam::Q16,
        protection_lower: 1,
        short_interleave: true,
        services: vec![music(mode), talk()],
        ..defaults(mode)
    }
}

#[test]
fn a_mode_b_multiplex_names_its_services_and_plays_the_first() {
    let iq = synth::drm::signal(two_services(Robustness::B), 9);
    let mut channel = channel(DrmMode::Auto, None);
    let out = run(&mut channel, &iq);
    let status = status(&out);
    assert!(status.locked, "{status:?}");
    assert_eq!(status.system, BroadcastSystem::Drm30);
    assert_eq!(status.services.len(), 2, "{status:?}");
    assert_eq!(status.services[0].label, "Rust Wave");
    assert_eq!(status.services[1].label, "Rust Talk");
    assert_eq!(status.services[0].language.as_deref(), Some("eng"));
    assert_eq!(status.service_id, Some(0x00D7A1));
    assert_eq!(status.label.as_deref(), Some("Rust Wave"));
    assert_eq!(status.code_rate.as_deref(), Some("16-QAM 0.62"));
    assert!(
        status
            .text
            .as_deref()
            .is_some_and(|text| text.starts_with("Mode B · 20 kHz"))
    );
    assert_eq!(status.frames_bad, 0, "{status:?}");
    assert_eq!(status.data_groups_bad, 0, "{status:?}");
    assert!(status.audio_frames_ok > 10, "{status:?}");
    assert_eq!(status.audio_frames_bad, 0, "{status:?}");
    assert_eq!(
        status.dynamic_label.as_deref(),
        Some("Now playing: a 700 Hz test tone")
    );
    assert!(out.audio_pcm.iter().any(|sample| sample.abs() > 0.01));
}

#[test]
fn choosing_a_service_switches_the_audio() {
    let iq = synth::drm::signal(two_services(Robustness::B), 6);
    let mut channel = channel(DrmMode::Drm30, Some(1));
    let out = run(&mut channel, &iq);
    let status = status(&out);
    assert!(status.locked, "{status:?}");
    assert_eq!(status.service_id, Some(0x00D7A2));
    assert_eq!(status.label.as_deref(), Some("Rust Talk"));
    assert!(status.services[1].selected);
    assert!(status.audio_frames_ok > 10, "{status:?}");
    assert_eq!(status.dynamic_label, None);
}

#[test]
fn mode_a_survives_noise_offset_and_echoes_with_unequal_protection() {
    let config = Config {
        protection_higher: 0,
        protection_lower: 1,
        higher_bytes: 60,
        ..defaults(Robustness::A)
    };
    let clean = synth::drm::signal(config, 6);
    let mut echoed = multipath(
        &clean,
        &[
            (0.0, Complex::new(1.0, 0.0)),
            (0.0011, Complex::new(0.0, 0.45)),
        ],
    );
    synth::shift(&mut echoed, 137.0, INPUT_RATE_HZ);
    let iq = at_snr(&echoed, 26.0, 11);
    let mut channel = channel(DrmMode::Drm30, None);
    let out = run(&mut channel, &iq);
    let status = status(&out);
    assert!(status.locked, "{status:?}");
    assert_eq!(status.code_rate.as_deref(), Some("64-QAM 0.6"));
    assert!(
        (status.frequency_error_hz - 137.0).abs() < 3.0,
        "{status:?}"
    );
    assert!(status.audio_frames_ok > 5, "{status:?}");
    assert_eq!(status.audio_frames_bad, 0, "{status:?}");
}

#[test]
fn drm_plus_decodes_with_noise_offset_and_echoes() {
    let clean = synth::drm::signal(defaults(Robustness::E), 6);
    let mut echoed = multipath(
        &clean,
        &[
            (0.0, Complex::new(1.0, 0.0)),
            (0.00008, Complex::new(-0.4, 0.2)),
        ],
    );
    synth::shift(&mut echoed, -1_234.0, INPUT_RATE_HZ);
    let iq = at_snr(&echoed, 22.0, 5);
    let mut channel = channel(DrmMode::Auto, None);
    let out = run(&mut channel, &iq);
    let status = status(&out);
    assert!(status.locked, "{status:?}");
    assert_eq!(status.system, BroadcastSystem::DrmPlus);
    assert_eq!(status.code_rate.as_deref(), Some("16-QAM 0.5"));
    assert!(
        (status.frequency_error_hz + 1_234.0).abs() < 10.0,
        "{status:?}"
    );
    assert!(status.audio_frames_ok > 20, "{status:?}");
    assert_eq!(status.audio_frames_bad, 0, "{status:?}");
    assert!(out.audio_pcm.iter().any(|sample| sample.abs() > 0.01));
}

#[test]
fn a_burst_of_interference_surfaces_as_errors() {
    let mut iq = synth::drm::signal(two_services(Robustness::B), 7);
    let start = iq.len() / 2;
    let burst = crate::testutil::complex_noise(3, 2.0, 60_000);
    for (sample, noise) in iq[start..start + burst.len()].iter_mut().zip(burst) {
        *sample += noise;
    }
    let mut channel = channel(DrmMode::Drm30, None);
    let out = run(&mut channel, &iq);
    let status = status(&out);
    assert!(
        status.frames_bad > 0 || status.audio_frames_bad > 0,
        "{status:?}"
    );
}

#[test]
fn noise_never_locks() {
    let mut channel = channel(DrmMode::Auto, None);
    let iq = crate::testutil::complex_noise(7, 0.5, 2 * INPUT_RATE_HZ as usize);
    let mut out = ChannelOutputs::default();
    channel.process(&iq, &mut out);
    assert!(
        out.events
            .iter()
            .all(|event| !matches!(event, DecoderEvent::Broadcast(status) if status.locked))
    );
}

#[test]
fn a_single_carrier_never_locks() {
    let mut channel = channel(DrmMode::Auto, None);
    let mut out = ChannelOutputs::default();
    channel.process(
        &vec![Complex::new(1.0, 0.0); 2 * INPUT_RATE_HZ as usize],
        &mut out,
    );
    assert!(
        out.events
            .iter()
            .all(|event| !matches!(event, DecoderEvent::Broadcast(status) if status.locked))
    );
}

#[test]
fn a_drifting_sample_clock_keeps_timing() {
    let clean = synth::drm::signal(defaults(Robustness::B), 8);
    let iq = synth::resample(&clean, INPUT_RATE_HZ, INPUT_RATE_HZ * (1.0 + 40e-6));
    let mut channel = channel(DrmMode::Drm30, None);
    let out = run(&mut channel, &iq);
    let status = status(&out);
    assert!(status.locked, "{status:?}");
    assert!(status.audio_frames_ok > 20, "{status:?}");
    assert_eq!(status.audio_frames_bad, 0, "{status:?}");
}

#[test]
fn every_robustness_mode_and_constellation_carries_fac_sdc_and_audio() {
    let cases = [
        (Robustness::A, 5, Qam::Q16, 1, false),
        (Robustness::A, 1, Qam::Q64, 2, true),
        (Robustness::B, 3, Qam::Q64, 0, true),
        (Robustness::B, 0, Qam::Q64, 3, false),
        (Robustness::C, 3, Qam::Q64, 1, false),
        (Robustness::C, 5, Qam::Q16, 0, true),
        (Robustness::D, 3, Qam::Q64, 1, true),
        (Robustness::D, 5, Qam::Q16, 1, false),
        (Robustness::E, 0, Qam::Q4, 1, false),
        (Robustness::E, 0, Qam::Q16, 3, false),
    ];
    for (mode, occupancy, msc, protection, short) in cases {
        let config = Config {
            occupancy,
            msc,
            protection_lower: protection,
            short_interleave: short,
            sdc_robust: occupancy % 2 == 1,
            ..defaults(mode)
        };
        let iq = synth::drm::signal(config, 6);
        let mut channel = channel(
            if mode.plus() {
                DrmMode::DrmPlus
            } else {
                DrmMode::Drm30
            },
            None,
        );
        let out = run(&mut channel, &iq);
        let status = status(&out);
        let case = format!("{mode:?} {occupancy} {msc:?} {protection}");
        assert!(status.locked, "{case}: {status:?}");
        let locked = channel.locked.as_ref().expect("locked");
        let fac = locked.receiver.fac.expect("a FAC");
        assert_eq!(fac.occupancy, occupancy, "{case}");
        assert_eq!(fac.msc, msc, "{case}");
        assert_eq!(fac.short_interleave, short && !mode.plus(), "{case}");
        assert_eq!(fac.service[0].map(|service| service.id), Some(0x00D7A1));
        assert_eq!(status.frames_bad, 0, "{case}: {status:?}");
        assert!(status.data_groups_ok > 0, "{case}: {status:?}");
        assert_eq!(status.data_groups_bad, 0, "{case}: {status:?}");
        assert!(status.audio_frames_ok > 0, "{case}: {status:?}");
        assert_eq!(status.audio_frames_bad, 0, "{case}: {status:?}");
        assert!(status.snr_db > 25.0, "{case}: {status:?}");
    }
}

#[test]
fn decoding_keeps_ahead_of_the_channel_rate() {
    let iq = synth::drm::signal(defaults(Robustness::B), 3);
    let mut channel = channel(DrmMode::Auto, None);
    let mut out = ChannelOutputs::default();
    let started = Instant::now();
    for block in iq.chunks(16_384) {
        out.reset();
        channel.process(block, &mut out);
    }
    let elapsed = started.elapsed().as_secs_f64();
    let seconds = iq.len() as f64 / INPUT_RATE_HZ;
    assert!(
        elapsed < realtime_budget(seconds),
        "{seconds:.2} s of DRM took {elapsed:.2} s"
    );
}
