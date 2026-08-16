use num_complex::Complex;
use sdrmm_wire::{ChannelParams, NfmParams, SstvMode, SstvParams};

use super::{
    modes::{BREAK_MS, LEADER_MS},
    *,
};
use crate::{
    testgen::{
        self,
        sstv::{Frame, bars, header, transmission},
    },
    testutil::{complex_noise, settings},
};

const RATE: f64 = INPUT_RATE_HZ;
const BLOCKS: [usize; 6] = [4_096, 1, 997, 65_536, 33, 12_288];

fn channel(p: SstvParams) -> SstvChannel {
    SstvChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Sstv(p)),
    )
    .expect("builds")
}

struct Received {
    images: Vec<DecodedImage>,
    events: Vec<SstvPicture>,
    progress: usize,
}

fn run(chan: &mut SstvChannel, iq: &[Complex<f32>], lens: &[usize]) -> Received {
    let mut out = ChannelOutputs::default();
    let mut received = Received {
        images: Vec::new(),
        events: Vec::new(),
        progress: 0,
    };
    let mut at = 0;
    for len in lens.iter().cycle() {
        if at >= iq.len() {
            break;
        }
        let end = (at + len).min(iq.len());
        out.reset();
        chan.process(&iq[at..end], &mut out);
        received.progress += out.video.len();
        received.images.append(&mut out.images);
        for event in &out.events {
            match event {
                DecoderEvent::Sstv(picture) => received.events.push(*picture),
                other => panic!("unexpected event {other:?}"),
            }
        }
        at = end;
    }
    received
}

fn tail(ms: f64) -> Vec<Complex<f32>> {
    testgen::silence(samples(ms, RATE) as usize)
}

fn decode(mode: SstvMode, frame: &Frame) -> DecodedImage {
    let mut iq = transmission(mode, frame, RATE);
    iq.extend_from_slice(&tail(2_000.0));
    let mut chan = channel(SstvParams::default());
    let received = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(
        received.images.len(),
        1,
        "{mode:?} produced no single image"
    );
    assert!(
        received.progress > 1,
        "{mode:?} sent no progressive updates"
    );
    received.images.into_iter().next().expect("one image")
}

fn mean_error(mode: SstvMode, sent: &Frame, got: &VideoPicture) -> f64 {
    let (width, height) = mode.size();
    assert_eq!((got.width, got.height), (width, height));
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for y in 0..height {
        for x in 0..width {
            let base = (usize::from(y) * usize::from(width) + usize::from(x)) * 3;
            for (channel, want) in sent.pixel(x, y).into_iter().enumerate() {
                sum += f64::from(got.rgb[base + channel].abs_diff(want));
                count += 1;
            }
        }
    }
    sum / count as f64
}

fn edges(mode: SstvMode) -> Vec<u16> {
    let (width, _) = mode.size();
    (1..8)
        .map(|bar| (u32::from(width) * bar / 8) as u16)
        .collect()
}

fn away_from_edges(mode: SstvMode, x: u16) -> bool {
    let guard = (mode.size().0 / 64).max(3);
    edges(mode).into_iter().all(|edge| x.abs_diff(edge) > guard)
}

fn interior_error(mode: SstvMode, sent: &Frame, got: &VideoPicture) -> f64 {
    let (width, height) = mode.size();
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for y in 0..height {
        for x in 0..width {
            if !away_from_edges(mode, x) {
                continue;
            }
            let base = (usize::from(y) * usize::from(width) + usize::from(x)) * 3;
            for (channel, want) in sent.pixel(x, y).into_iter().enumerate() {
                sum += f64::from(got.rgb[base + channel].abs_diff(want));
                count += 1;
            }
        }
    }
    sum / count as f64
}

#[test]
fn the_track_holds_the_longest_line_plus_a_write_chunk() {
    let longest = samples(modes::longest_line_ms(), RATE) as usize;
    let search = samples(SEARCH_MS, RATE) as usize;
    assert!(
        longest + 2 * search + WRITE_CHUNK < TRACK_CAPACITY,
        "a {longest}-sample line plus a {WRITE_CHUNK}-sample write does not fit {TRACK_CAPACITY}"
    );
}

#[test]
fn a_vis_header_names_its_mode() {
    for &mode in &SstvMode::ALL {
        let mut iq = header(mode, RATE);
        iq.extend_from_slice(&tail(50.0));
        let mut chan = channel(SstvParams::default());
        let mut out = ChannelOutputs::default();
        chan.process(&iq, &mut out);
        assert!(
            chan.picture.active,
            "{mode:?} header did not start a picture"
        );
        assert_eq!(chan.picture.timing.mode, mode);
    }
}

#[test]
fn a_header_with_broken_parity_starts_nothing() {
    let mut iq = header(SstvMode::MartinM1, RATE);
    let bit = samples(VIS_BIT_MS, RATE) as usize;
    let leaders = samples(LEADER_MS * 2.0 + BREAK_MS, RATE) as usize;
    let parity = leaders + bit * 8;
    let flipped = testgen::sstv::header(SstvMode::MartinM2, RATE);
    iq[parity..parity + bit].copy_from_slice(&flipped[parity..parity + bit]);
    iq.extend_from_slice(&tail(50.0));

    let mut chan = channel(SstvParams::default());
    let mut out = ChannelOutputs::default();
    chan.process(&iq, &mut out);
    assert!(
        !chan.picture.active,
        "a corrupted VIS still started a picture"
    );
}

#[test]
fn every_mode_decodes_its_own_transmission() {
    for &mode in &SstvMode::ALL {
        let sent = bars(mode);
        let image = decode(mode, &sent);
        assert!(image.complete, "{mode:?} did not complete");
        assert_eq!(image.source, SOURCE);
        assert_eq!(image.mode, mode.label());
        assert_eq!(image.lines, mode.size().1);
        let error = interior_error(mode, &sent, &image.picture);
        assert!(error < 12.0, "{mode:?} mean interior error {error:.1}/255");
    }
}

#[test]
fn a_decoded_picture_reports_its_mode_and_size() {
    let mode = SstvMode::MartinM1;
    let mut iq = transmission(mode, &bars(mode), RATE);
    iq.extend_from_slice(&tail(2_000.0));
    let mut chan = channel(SstvParams::default());
    let received = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(received.events.len(), 1);
    let event = received.events[0];
    assert_eq!(event.mode, mode);
    assert_eq!((event.width, event.height), mode.size());
    assert_eq!(event.lines, 256);
    assert!(event.complete);
    assert_eq!(event.seq, 1);
    let expected = timing(mode).seconds() * 1_000.0;
    let actual = f64::from(event.duration_ms);
    assert!(
        (actual - expected).abs() < 2_000.0,
        "reported {actual} ms against a {expected} ms transmission"
    );
}

#[test]
fn ragged_block_splits_decode_identically() {
    let mode = SstvMode::Robot36;
    let sent = bars(mode);
    let mut iq = transmission(mode, &sent, RATE);
    iq.extend_from_slice(&tail(2_000.0));

    let whole = run(&mut channel(SstvParams::default()), &iq, &[iq.len()]);
    let ragged = run(&mut channel(SstvParams::default()), &iq, &BLOCKS);
    assert_eq!(whole.images.len(), 1);
    assert_eq!(ragged.images.len(), 1);
    assert_eq!(whole.images[0].picture, ragged.images[0].picture);
}

#[test]
fn a_forced_mode_ignores_the_transmitted_vis() {
    let sent = bars(SstvMode::MartinM1);
    let mut iq = transmission(SstvMode::MartinM1, &sent, RATE);
    iq.extend_from_slice(&tail(2_000.0));
    let mut chan = channel(SstvParams {
        mode: Some(SstvMode::MartinM2),
        ..SstvParams::default()
    });
    let received = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(received.images.len(), 1);
    assert_eq!(received.images[0].mode, SstvMode::MartinM2.label());
}

#[test]
fn a_transmission_cut_short_is_kept_as_a_partial_picture() {
    let mode = SstvMode::Robot36;
    let sent = bars(mode);
    let full = transmission(mode, &sent, RATE);
    let half = full.len() / 2;
    let mut iq = full[..half].to_vec();
    iq.extend_from_slice(&testgen::silence(samples(20_000.0, RATE) as usize));

    let mut chan = channel(SstvParams::default());
    let received = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(received.images.len(), 1);
    let image = &received.images[0];
    assert!(
        !image.complete,
        "a truncated picture claimed to be complete"
    );
    assert!(
        (100..200).contains(&image.lines),
        "kept {} lines of a half transmission",
        image.lines
    );
    assert!(!received.events[0].complete);
}

#[test]
fn dropping_partials_keeps_only_finished_pictures() {
    let mode = SstvMode::Robot36;
    let full = transmission(mode, &bars(mode), RATE);
    let mut iq = full[..full.len() / 2].to_vec();
    iq.extend_from_slice(&testgen::silence(samples(20_000.0, RATE) as usize));

    let mut chan = channel(SstvParams {
        keep_partial: false,
        ..SstvParams::default()
    });
    let received = run(&mut chan, &iq, &BLOCKS);
    assert!(received.images.is_empty(), "a partial survived the setting");
}

#[test]
fn a_second_transmission_follows_the_first() {
    let mode = SstvMode::Robot36;
    let sent = bars(mode);
    let mut iq = transmission(mode, &sent, RATE);
    iq.extend_from_slice(&tail(500.0));
    iq.extend(transmission(mode, &sent, RATE));
    iq.extend_from_slice(&tail(2_000.0));

    let mut chan = channel(SstvParams::default());
    let received = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(received.images.len(), 2);
    assert!(received.images.iter().all(|image| image.complete));
    assert_eq!(received.events[0].seq, 1);
    assert_eq!(received.events[1].seq, 2);
}

#[test]
fn a_slanted_clock_still_lands_in_frame() {
    let mode = SstvMode::MartinM2;
    let sent = bars(mode);
    let straight = transmission(mode, &sent, RATE);
    let slanted = testgen::resample(&straight, RATE, RATE * 1.0005);
    let mut iq = slanted;
    iq.extend_from_slice(&tail(2_000.0));

    let mut chan = channel(SstvParams::default());
    let received = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(received.images.len(), 1);
    let error = interior_error(mode, &sent, &received.images[0].picture);
    assert!(error < 20.0, "slanted mean interior error {error:.1}/255");

    let mut chan = channel(SstvParams {
        slant_correction: false,
        ..SstvParams::default()
    });
    let free = run(&mut chan, &iq, &BLOCKS);
    assert_eq!(free.images.len(), 1);
    let uncorrected = interior_error(mode, &sent, &free.images[0].picture);
    assert!(
        uncorrected > error,
        "correction made it worse: {error:.1} against {uncorrected:.1}"
    );
}

#[test]
fn decodes_through_additive_noise() {
    let mode = SstvMode::MartinM2;
    let sent = bars(mode);
    let mut iq = transmission(mode, &sent, RATE);
    testgen::add_noise(&mut iq, 0xabad_1dea, 0.25);
    let mut filtered = Vec::new();
    channel_filter(&SstvParams::default())
        .expect("filter")
        .process(&iq, &mut filtered);
    filtered.extend_from_slice(&tail(2_000.0));

    let mut chan = channel(SstvParams::default());
    let received = run(&mut chan, &filtered, &BLOCKS);
    assert_eq!(received.images.len(), 1);
    let error = interior_error(mode, &sent, &received.images[0].picture);
    assert!(error < 25.0, "noisy mean interior error {error:.1}/255");
}

#[test]
fn pure_noise_decodes_to_nothing() {
    for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
        let noise = complex_noise(seed, 0.4, 400_000);
        let mut chan = channel(SstvParams::default());
        let received = run(&mut chan, &noise, &BLOCKS);
        assert!(
            received.images.is_empty(),
            "seed {seed:#x} produced {} images",
            received.images.len()
        );
    }
}

#[test]
fn a_grey_wedge_survives_the_round_trip() {
    let mode = SstvMode::MartinM1;
    let (width, height) = mode.size();
    let mut sent = Frame::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let level = (u32::from(x) * 255 / u32::from(width - 1)) as u8;
            sent.set(x, y, [level, level, level]);
        }
    }
    let image = decode(mode, &sent);
    let error = mean_error(mode, &sent, &image.picture);
    assert!(error < 6.0, "grey wedge mean error {error:.1}/255");
}

#[test]
fn retuning_drops_the_picture_in_flight() {
    let mode = SstvMode::MartinM1;
    let iq = transmission(mode, &bars(mode), RATE);
    let mut chan = channel(SstvParams::default());
    let mut out = ChannelOutputs::default();
    chan.process(&iq[..iq.len() / 4], &mut out);
    assert!(chan.picture.active);
    chan.retuned();
    assert!(!chan.picture.active);
    out.reset();
    chan.process(&iq[iq.len() / 4..], &mut out);
    assert!(out.images.is_empty(), "a retuned channel still emitted");
}

#[test]
fn mismatched_params_variant_is_rejected() {
    let mut chan = channel(SstvParams::default());
    let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
    assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    let built = SstvChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Nfm(NfmParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}

#[test]
fn wrong_input_rate_is_rejected() {
    let built = SstvChannel::new(
        ChannelCtx {
            input_rate: 48_000.0,
        },
        settings(ChannelParams::Sstv(SstvParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}

#[test]
fn changing_the_forced_mode_abandons_the_picture() {
    let mode = SstvMode::MartinM1;
    let iq = transmission(mode, &bars(mode), RATE);
    let mut chan = channel(SstvParams::default());
    let mut out = ChannelOutputs::default();
    chan.process(&iq[..iq.len() / 4], &mut out);
    assert!(chan.picture.active);
    chan.apply(settings(ChannelParams::Sstv(SstvParams {
        mode: Some(SstvMode::ScottieS1),
        ..SstvParams::default()
    })))
    .expect("applies");
    assert!(!chan.picture.active);
}
