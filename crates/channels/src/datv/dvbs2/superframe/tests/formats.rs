use super::*;

const FREQUENCY: f32 = 0.001;
const NOISE: f32 = 0.25;
const QUIET: f32 = 0.15;

fn legacy(modcod: u8, short: bool) -> Coding {
    Coding::Legacy { modcod, short }
}

#[test]
fn format_two_bundles_round_trip_through_noise_and_offset() {
    let mut source = Source::new();
    let bundles = vec![
        source.bundle(2, 4),
        source.bundle(2, 66),
        source.bundle(2, 0),
        source.bundle(2, 108),
        source.bundle(2, 32 | 4),
    ];
    let mut signal = Transmitter::new(Codes::default()).bundled(2, &bundles);
    assert_eq!(signal.len(), LENGTH);
    impair(&mut signal, FREQUENCY, NOISE, 5);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_eq!(decoder.superframe_format(), Some(2));
    assert_delivered(&source, &decoder, &output, "format 2");
    assert_eq!(decoder.metrics.frames_ok, 2 + 2 + 2 + 8);
}

#[test]
fn format_three_bundles_round_trip_through_noise_and_offset() {
    let mut source = Source::new();
    let bundles = vec![
        source.bundle(3, 32 | 4),
        source.bundle(3, 64),
        source.bundle(3, 32),
        source.bundle(3, 77),
        source.bundle(3, 82),
    ];
    let mut signal = Transmitter::new(Codes::default()).bundled(3, &bundles);
    assert_eq!(signal.len(), LENGTH);
    impair(&mut signal, FREQUENCY, QUIET, 7);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_eq!(decoder.superframe_format(), Some(3));
    assert_delivered(&source, &decoder, &output, "format 3");
    assert_eq!(decoder.metrics.frames_ok, 2 + 1 + 4 + 5);
}

#[test]
fn format_four_frames_span_superframes_with_spreading_and_robust_headers() {
    let mut source = Source::new();
    let mut frames = vec![
        source.plframe(legacy(4, true), 1),
        source.plframe(legacy(12, true), 1),
    ];
    frames.extend(std::iter::repeat_n(Plframe::dummy(), 159));
    frames.push(source.plframe(legacy(18, false), 1));
    frames.push(source.plframe(legacy(1, true), 2));
    frames.push(source.plframe(Coding::Extended { code: 216 }, 1));
    let codes = Codes {
        trailer: 50,
        ..Codes::default()
    };
    let options = Options {
        protection: Protection::Robust,
        ..Options::default()
    };
    let mut signal = Transmitter::new(codes).flexible(4, &frames, options);
    assert_eq!(signal.len(), 2 * LENGTH);
    impair(&mut signal, FREQUENCY, NOISE, 9);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_delivered(&source, &decoder, &output, "format 4");
    let identity = decoder.superframe_identity().expect("an identity");
    assert_eq!(identity.format, Some(4));
    assert_eq!(identity.trailer, Some(50));
    assert_eq!(identity.pilot, Some(0));
}

#[test]
fn format_four_without_pilots_tracks_on_the_headers() {
    let mut source = Source::new();
    let mut frames = Vec::new();
    for _ in 0..4 {
        frames.push(source.plframe(legacy(4, true), 1));
    }
    let options = Options {
        pilots: false,
        protection: Protection::Standard,
        ..Options::default()
    };
    let mut signal = Transmitter::new(Codes::default()).flexible(4, &frames, options);
    impair(&mut signal, FREQUENCY, NOISE, 13);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_delivered(&source, &decoder, &output, "format 4 without pilots");
}

#[test]
fn format_five_fragments_frames_over_superframes_and_dwells() {
    let mut source = Source::new();
    let mut frames = Vec::new();
    for round in 0..3 {
        frames.push(source.plframe(legacy(4, true), 1));
        frames.push(source.plframe(legacy(1, true), if round == 1 { 5 } else { 2 }));
        frames.push(source.plframe(
            Coding::Robust {
                carrier: Carrier::Qpsk,
                rate: Rate::R1_5,
                frame: Frame::Medium,
            },
            2,
        ));
    }
    let options = Options {
        protection: Protection::MostRobust,
        periods: 20,
        dwell: Some(Dwell {
            superframes: 3,
            cut: 150,
            extra: 90,
            gap: 4000,
        }),
        ..Options::default()
    };
    let mut signal = Transmitter::new(Codes::default()).flexible(5, &frames, options);
    impair(&mut signal, FREQUENCY, NOISE, 17);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_eq!(decoder.superframe_format(), Some(5));
    assert_delivered(&source, &decoder, &output, "format 5");
}

#[test]
fn format_five_continuous_superframes_use_the_efficient_header() {
    let mut source = Source::new();
    let frames: Vec<Plframe> = (0..10)
        .map(|index| source.plframe(legacy(if index % 2 == 0 { 12 } else { 18 }, true), 1))
        .collect();
    let options = Options {
        protection: Protection::Efficient,
        periods: 10,
        ..Options::default()
    };
    let mut signal = Transmitter::new(Codes::default()).flexible(5, &frames, options);
    impair(&mut signal, FREQUENCY, NOISE, 19);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_delivered(&source, &decoder, &output, "format 5 continuous");
}

#[test]
fn format_six_superframes_end_with_their_last_frame() {
    let mut source = Source::new();
    let frames = vec![
        source.plframe(legacy(4, true), 1),
        source.plframe(legacy(12, true), 1),
        source.plframe(legacy(1, true), 2),
        source.plframe(legacy(4, true), 1),
        source.plframe(Coding::Extended { code: 236 }, 1),
        source.plframe(legacy(2, true), 1),
    ];
    let codes = Codes {
        sosf: 21,
        ..Codes::default()
    };
    let options = Options {
        protection: Protection::Robust,
        frames: 2,
        dwell: Some(Dwell {
            superframes: 2,
            cut: 0,
            extra: 45,
            gap: 3000,
        }),
        ..Options::default()
    };
    let mut signal = Transmitter::new(codes).flexible(6, &frames, options);
    impair(&mut signal, FREQUENCY, NOISE, 23);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_delivered(&source, &decoder, &output, "format 6");
    assert_eq!(
        decoder.superframe_identity().map(|identity| identity.sosf),
        Some(21)
    );
}

#[test]
fn format_seven_superframes_carry_whole_frames() {
    let mut source = Source::new();
    let frames: Vec<Plframe> = [4u8, 12, 18, 24, 6, 13, 4]
        .iter()
        .map(|&modcod| source.plframe(legacy(modcod, true), 1))
        .collect();
    let options = Options {
        frames: 3,
        dwell: Some(Dwell {
            superframes: 1,
            cut: 0,
            extra: 0,
            gap: 2500,
        }),
        ..Options::default()
    };
    let mut signal = Transmitter::new(Codes::default()).flexible(7, &frames, options);
    impair(&mut signal, FREQUENCY, QUIET, 29);
    let (decoder, output) = receive_flushed(&signal, Settings::default());
    assert_eq!(decoder.superframe_format(), Some(7));
    assert_delivered(&source, &decoder, &output, "format 7");
}

fn coded_signal(codes: Codes) -> (Source, Vec<Complex<f32>>) {
    let mut source = Source::new();
    let frames: Vec<Plframe> = (0..4).map(|_| source.plframe(legacy(4, true), 1)).collect();
    let options = Options {
        frames: 2,
        ..Options::default()
    };
    let mut signal = Transmitter::new(codes).flexible(7, &frames, options);
    impair(&mut signal, FREQUENCY, NOISE, 31);
    (source, signal)
}

#[test]
fn configured_scrambling_codes_and_walsh_rows_are_received() {
    let codes = Codes {
        reference: 123_456,
        payload: 654_321,
        sosf: 37,
        pilot: 9,
        trailer: 0,
    };
    let (source, signal) = coded_signal(codes);
    let settings = Settings {
        reference: codes.reference,
        payload: codes.payload,
        search: false,
    };
    let (decoder, output) = receive_flushed(&signal, settings);
    assert_delivered(&source, &decoder, &output, "configured codes");
    let identity = decoder.superframe_identity().expect("an identity");
    assert_eq!(identity.sosf, 37);
    assert_eq!(identity.pilot, Some(9));
    assert_eq!((identity.reference, identity.payload), (123_456, 654_321));

    let (blind, nothing) = receive_flushed(&signal, Settings::default());
    assert_eq!(blind.metrics.frames_ok, 0);
    assert!(nothing.packets.is_empty());
    assert_eq!(blind.superframe_format(), None);
}

#[test]
fn unknown_scrambling_codes_are_found_by_searching() {
    let codes = Codes {
        reference: 777,
        payload: 1_048_000,
        sosf: 200,
        pilot: 31,
        trailer: 0,
    };
    let (source, signal) = coded_signal(codes);
    let settings = Settings {
        search: true,
        ..Settings::default()
    };
    let (decoder, output) = receive_flushed(&signal, settings);
    assert_delivered(&source, &decoder, &output, "searched codes");
    let identity = decoder.superframe_identity().expect("an identity");
    assert_eq!((identity.reference, identity.payload), (777, 1_048_000));
    assert_eq!((identity.sosf, identity.pilot), (200, Some(31)));
}

#[test]
fn zz_probe() {
    for format in [6u8, 7] {
        let mut source = Source::new();
        let frames: Vec<Plframe> = (0..12)
            .map(|_| source.plframe(legacy(4, false), 1))
            .collect();
        let options = Options {
            frames: 4,
            ..Options::default()
        };
        let mut signal = Transmitter::new(Codes::default()).flexible(format, &frames, options);
        impair(&mut signal, FREQUENCY, NOISE, 3);
        let (decoder, output) = receive_flushed(&signal, Settings::default());
        eprintln!(
            "format {format}: {} of {} {:?}",
            output.packets.len(),
            source.sent.len(),
            decoder.metrics
        );
    }
}
