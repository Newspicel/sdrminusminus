use super::*;
use crate::testgen;

const RATE: f64 = 1_024_000.0;
const CENTER: f64 = 145_000_000.0;

fn noise(seconds: f64, seed: u32) -> Vec<Complex<f32>> {
    crate::testutil::complex_noise(seed, 0.002, (RATE * seconds) as usize)
}

fn run(monitor: &mut SpectrumMonitor, samples: &[Complex<f32>], start: u64) -> Vec<MonitorOutput> {
    let mut output = Vec::new();
    for (index, block) in samples.chunks(8192).enumerate() {
        output.extend(monitor.process(block, start + (index * 8192) as u64));
    }
    output
}

fn monitor() -> SpectrumMonitor {
    SpectrumMonitor::new(RATE, CENTER, SpectrumMonitorNode::default()).unwrap()
}

fn transmissions(output: &[MonitorOutput]) -> Vec<&Transmission> {
    output
        .iter()
        .filter_map(|out| match &out.event {
            DecoderEvent::Transmission(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn quiet_wideband_stream_does_not_invent_transmissions() {
    let mut monitor = monitor();
    assert!(run(&mut monitor, &noise(0.5, 41), 0).is_empty());
}

fn uncertain_noise(seconds: f64) -> Vec<Complex<f32>> {
    let mut iq = noise(seconds, 80);
    let mut filtered = Complex::new(0.0, 0.0);
    let random = crate::testutil::complex_noise(81, 0.6, iq.len());
    for (i, (sample, random)) in iq.iter_mut().zip(random).enumerate() {
        filtered = filtered * 0.95 + random * 0.05;
        *sample += filtered
            * Complex::from_polar(
                1.0,
                (std::f64::consts::TAU * 250_000.0 * i as f64 / RATE)
                    .rem_euclid(std::f64::consts::TAU) as f32,
            );
    }
    iq
}

#[test]
fn uncertain_noise_is_rejected_before_decoding_or_recording() {
    let iq = uncertain_noise(0.3);
    let mut permissive = SpectrumMonitor::new(
        RATE,
        CENTER,
        SpectrumMonitorNode {
            min_confidence: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let output = run(&mut permissive, &iq, 0);
    let detected = transmissions(&output);
    assert!(!detected.is_empty());
    assert!(
        detected.iter().all(|t| t.signal.confidence < 0.7),
        "{detected:?}"
    );
    let mut filtered = monitor();
    assert!(run(&mut filtered, &iq, 0).is_empty());
    assert!(filtered.tracks.is_empty());
    assert!(
        filtered
            .finish(TransmissionState::Completed, None)
            .is_empty()
    );
}

#[test]
fn invalid_confidence_is_rejected_before_capture() {
    for min_confidence in [-0.1, 1.1, f32::NAN] {
        assert!(
            SpectrumMonitor::new(
                RATE,
                CENTER,
                SpectrumMonitorNode {
                    min_confidence,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}

#[test]
fn concurrent_am_and_fm_outside_identifier_span_have_separate_audio_events() {
    let mut iq = noise(0.7, 42);
    let mut audio = testgen::tone_audio(900.0, 0.6, RATE, iq.len());
    for (index, sample) in audio.iter_mut().enumerate() {
        *sample += 0.2 * (std::f64::consts::TAU * 1437.0 * index as f64 / RATE).sin() as f32;
    }
    let mut fm = testgen::fm_modulate(&audio, 3000.0, RATE);
    testgen::shift(&mut fm, 250_000.0, RATE);
    let mut am: Vec<_> = audio
        .iter()
        .map(|sample| Complex::new(0.4 * (1.0 + sample), 0.0))
        .collect();
    testgen::shift(&mut am, -250_000.0, RATE);
    for ((sample, fm), am) in iq.iter_mut().zip(fm).zip(am) {
        *sample += fm + am;
    }
    let mut monitor = SpectrumMonitor::new(
        RATE,
        CENTER,
        SpectrumMonitorNode {
            min_confidence: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let mut output = run(&mut monitor, &iq, 0);
    output.extend(run(&mut monitor, &noise(0.5, 43), iq.len() as u64));
    let completed: Vec<_> = output.iter().filter(|out| matches!(&out.event, DecoderEvent::Transmission(t) if t.state == TransmissionState::Completed)).collect();
    for offset in [-250_000.0, 250_000.0] {
        let found = completed
            .iter()
            .find(|out| (out.frequency_hz - CENTER - offset).abs() < 4000.0)
            .unwrap_or_else(|| panic!("missing {offset}: {:?}", transmissions(&output)));
        assert!(
            !found.audio.is_empty(),
            "no audio at {offset}: {:?}",
            found.event
        );
        assert!(found.audio.iter().any(|sample| sample.unsigned_abs() > 10));
    }
}

#[test]
fn buffered_pager_decodes_the_first_message_and_retains_its_origin() {
    let pages = [testgen::pocsag::Page {
        address: 1_234_567,
        function: 3,
        text: "MONITOR FIRST FRAME".to_owned(),
        numeric: false,
    }];
    let mut iq = testgen::pocsag::transmission(&pages, 1200, 4500.0, RATE);
    testgen::shift(&mut iq, 300_000.0, RATE);
    testgen::add_noise(&mut iq, 44, 0.002);
    let mut monitor = monitor();
    let mut output = run(&mut monitor, &iq, 0);
    output.extend(run(&mut monitor, &noise(0.5, 45), iq.len() as u64));
    let decoded = output.iter().find(|out| matches!(&out.event, DecoderEvent::Pocsag(page) if page.text.contains("MONITOR FIRST FRAME"))).unwrap_or_else(|| panic!("pager not decoded: {:?}", transmissions(&output)));
    assert!(decoded.transmission > 0);
    assert!((decoded.frequency_hz - CENTER - 300_000.0).abs() < 3000.0);
    assert!(
        transmissions(&output)
            .iter()
            .any(|t| t.id == decoded.transmission && t.start_sample == 0)
    );
}

#[test]
fn a_capture_gap_interrupts_instead_of_joining_unrelated_samples() {
    let mut iq = noise(0.3, 46);
    for (i, sample) in iq.iter_mut().enumerate() {
        *sample += Complex::from_polar(
            0.5,
            (std::f64::consts::TAU * 200_000.0 * i as f64 / RATE) as f32,
        );
    }
    let mut monitor = monitor();
    let first = run(&mut monitor, &iq, 0);
    assert!(!transmissions(&first).is_empty());
    let gap = run(&mut monitor, &noise(0.2, 47), iq.len() as u64 + 8192);
    assert!(
        transmissions(&gap)
            .iter()
            .any(|t| t.state == TransmissionState::Interrupted && t.error.is_some())
    );
    assert!(
        transmissions(&gap)
            .iter()
            .any(|t| t.state == TransmissionState::Problem)
    );
}

#[test]
fn audio_can_be_disabled_without_disabling_events() {
    let mut monitor = SpectrumMonitor::new(
        RATE,
        CENTER,
        SpectrumMonitorNode {
            record_audio: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut iq = noise(0.3, 48);
    for (i, sample) in iq.iter_mut().enumerate() {
        let amplitude =
            0.4 * (1.0 + 0.6 * (std::f64::consts::TAU * 900.0 * i as f64 / RATE).sin() as f32);
        *sample += Complex::from_polar(
            amplitude,
            (std::f64::consts::TAU * 200_000.0 * i as f64 / RATE) as f32,
        );
    }
    let mut output = run(&mut monitor, &iq, 0);
    output.extend(monitor.finish(TransmissionState::Interrupted, None));
    assert!(!transmissions(&output).is_empty());
    assert!(output.iter().all(|out| out.audio.is_empty()));
}

#[test]
fn a_carrier_that_starts_modulating_is_reidentified_automatically() {
    let mut iq = noise(0.9, 51);
    for (i, sample) in iq.iter_mut().enumerate() {
        let t = i as f64 / RATE;
        let amplitude = if t < 0.15 {
            0.5
        } else {
            0.5 + 0.3 * (std::f64::consts::TAU * 1100.0 * t).sin() as f32
        };
        *sample += Complex::from_polar(
            amplitude,
            (std::f64::consts::TAU * 250_000.0 * t).rem_euclid(std::f64::consts::TAU) as f32,
        );
    }
    let mut monitor = SpectrumMonitor::new(
        RATE,
        CENTER,
        SpectrumMonitorNode {
            min_confidence: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let mut output = run(&mut monitor, &iq, 0);
    output.extend(run(&mut monitor, &noise(0.4, 52), iq.len() as u64));
    assert!(
        output.iter().any(|out| !out.audio.is_empty()),
        "{:?}",
        transmissions(&output)
    );
}

#[test]
fn more_than_thirty_two_signals_reports_capacity_loss() {
    let mut iq = noise(0.1, 53);
    for offset in (0..34).map(|i| -450_000.0 + i as f64 * 25_000.0) {
        for (i, sample) in iq.iter_mut().enumerate() {
            *sample += Complex::from_polar(
                0.2,
                (std::f64::consts::TAU * offset * i as f64 / RATE).rem_euclid(std::f64::consts::TAU)
                    as f32,
            );
        }
    }
    let mut monitor = monitor();
    let output = run(&mut monitor, &iq, 0);
    assert_eq!(monitor.tracks.len(), 32);
    assert!(
        transmissions(&output)
            .iter()
            .any(|t| t.state == TransmissionState::Problem)
    );
}

#[test]
fn a_short_final_window_is_examined_when_the_stream_finishes() {
    let mut iq = noise(0.05, 54);
    for (i, sample) in iq.iter_mut().enumerate() {
        *sample += Complex::from_polar(
            0.5,
            (std::f64::consts::TAU * 250_000.0 * i as f64 / RATE).rem_euclid(std::f64::consts::TAU)
                as f32,
        );
    }
    let mut monitor = SpectrumMonitor::new(
        RATE,
        CENTER,
        SpectrumMonitorNode {
            min_confidence: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run(&mut monitor, &iq, 0).is_empty());
    let output = monitor.finish(TransmissionState::Completed, None);
    assert!(
        transmissions(&output)
            .iter()
            .any(|t| t.state == TransmissionState::Completed)
    );
}

#[test]
fn continuous_audio_is_segmented_without_loss_or_buffer_overflow() {
    let rate = 48_000.0;
    let mut monitor = SpectrumMonitor::new(rate, CENTER, SpectrumMonitorNode::default()).unwrap();
    let mut iq = crate::testutil::complex_noise(55, 0.002, 4800);
    for (i, sample) in iq.iter_mut().enumerate() {
        let t = i as f64 / rate;
        let amplitude = 0.5 + 0.3 * (std::f64::consts::TAU * 900.0 * t).sin() as f32;
        *sample += Complex::from_polar(
            amplitude,
            (std::f64::consts::TAU * 10_000.0 * t).rem_euclid(std::f64::consts::TAU) as f32,
        );
    }
    let mut output = Vec::new();
    for block in 0..650 {
        output.extend(monitor.process(&iq, block * 4800));
    }
    output.extend(monitor.finish(TransmissionState::Completed, None));
    let clips: Vec<_> = output.iter().filter(|out| !out.audio.is_empty()).collect();
    assert_eq!(clips.len(), 3, "{:?}", transmissions(&output));
    assert!(
        clips
            .iter()
            .all(|out| out.transmission == clips[0].transmission)
    );
    assert!(transmissions(&output).iter().all(|t| t.error.is_none()));
    assert_eq!(
        clips.iter().map(|out| out.audio.len()).sum::<usize>(),
        65 * 8000
    );
    let segments: Vec<_> = transmissions(&output)
        .into_iter()
        .filter(|t| t.id == clips[0].transmission && t.state != TransmissionState::Started)
        .collect();
    assert_eq!(segments[0].state, TransmissionState::Continued);
    assert_eq!(segments[1].state, TransmissionState::Continued);
    assert_eq!(segments[2].state, TransmissionState::Completed);
    assert_eq!(segments[0].end_sample, segments[1].start_sample);
    assert_eq!(segments[1].end_sample, segments[2].start_sample);
}

#[test]
fn an_unconfirmed_track_closes_when_confidence_falls() {
    let mut iq = noise(0.1, 82);
    for (i, sample) in iq.iter_mut().enumerate() {
        *sample += Complex::from_polar(
            0.5,
            (std::f64::consts::TAU * 250_000.0 * i as f64 / RATE).rem_euclid(std::f64::consts::TAU)
                as f32,
        );
    }
    let mut monitor = monitor();
    let started = run(&mut monitor, &iq, 0);
    let id = transmissions(&started).first().unwrap().id;
    let output = run(&mut monitor, &uncertain_noise(0.8), iq.len() as u64);
    assert!(
        transmissions(&output)
            .iter()
            .any(|t| t.id == id && t.state == TransmissionState::Completed)
    );
    assert!(monitor.tracks.is_empty());
}
