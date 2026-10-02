use num_complex::Complex;
use sdrmm_wire::{AptImage, AptParams, AvhrrChannel, ChannelParams, NfmParams};

use super::{
    geometry::{LINE_WORDS, Side},
    *,
};
use crate::{
    VideoPicture,
    synth::{
        self,
        apt::{doppler, image_a, image_b, transmission},
    },
    testutil::{complex_noise, settings},
};

const RATE: f64 = INPUT_RATE_HZ;
const BLOCKS: [usize; 6] = [4_096, 1, 997, 65_536, 33, 12_288];
const EDGE_GUARD: usize = 12;

fn channel(p: AptParams) -> AptChannel {
    AptChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Apt(p)),
    )
    .expect("builds")
}

struct Received {
    images: Vec<DecodedImage>,
    events: Vec<AptImage>,
    progress: usize,
}

fn run(chan: &mut AptChannel, iq: &[Complex<f32>], lens: &[usize]) -> Received {
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
                DecoderEvent::Apt(image) => received.events.push(*image),
                other => panic!("unexpected event {other:?}"),
            }
        }
        at = end;
    }
    received
}

fn tail(seconds: f64) -> Vec<Complex<f32>> {
    synth::silence((seconds * RATE) as usize)
}

fn pass(lines: u16) -> Vec<Complex<f32>> {
    let mut iq = transmission(lines, AvhrrChannel::Ch2, AvhrrChannel::Ch4, RATE);
    iq.extend_from_slice(&tail(10.0));
    iq
}

fn decode_one(iq: &[Complex<f32>]) -> (DecodedImage, AptImage) {
    let received = run(&mut channel(AptParams::default()), iq, &BLOCKS);
    assert_eq!(received.images.len(), 1, "expected one picture");
    assert!(received.progress > 1, "no progressive updates");
    (received.images[0].clone(), received.events[0])
}

fn error_at(picture: &VideoPicture, offset: u16) -> f64 {
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for y in 0..picture.height {
        let line = y + offset;
        for (side, sent) in [(Side::A, image_a as fn(u16, u16) -> u8), (Side::B, image_b)] {
            let range = side.image();
            for word in range.start + EDGE_GUARD..range.end - EDGE_GUARD {
                let got = picture.luma[usize::from(y) * LINE_WORDS + word];
                let want = sent(line, (word - range.start) as u16);
                sum += f64::from(got.abs_diff(want));
                count += 1;
            }
        }
    }
    sum / count as f64
}

fn pixel_error(picture: &VideoPicture) -> f64 {
    (0..4)
        .map(|offset| error_at(picture, offset))
        .fold(f64::MAX, f64::min)
}

#[test]
fn a_clean_pass_round_trips_its_pixels() {
    let (image, event) = decode_one(&pass(160));
    assert!(image.complete);
    assert_eq!(image.source, SOURCE);
    assert!((157..=160).contains(&image.lines), "{} lines", image.lines);
    assert_eq!(image.picture.width, LINE_WORDS as u16);
    assert_eq!(image.picture.rgb.len(), image.picture.luma.len() * 3);
    let error = pixel_error(&image.picture);
    assert!(error < 2.0, "mean error {error:.1}/255");
    assert_eq!(event.seq, 1);
    assert_eq!(event.lines, image.lines);
    let expected = f64::from(event.lines) * 500.0;
    assert!((f64::from(event.duration_ms) - expected).abs() < 1_000.0);
}

#[test]
fn telemetry_names_the_avhrr_channels() {
    let mut iq = transmission(160, AvhrrChannel::Ch3b, AvhrrChannel::Ch5, RATE);
    iq.extend_from_slice(&tail(10.0));
    let (image, event) = decode_one(&iq);
    assert_eq!(event.channel_a, Some(AvhrrChannel::Ch3b));
    assert_eq!(event.channel_b, Some(AvhrrChannel::Ch5));
    assert_eq!(image.mode, "APT 3B/5");
}

#[test]
fn ragged_block_splits_decode_identically() {
    let iq = pass(96);
    let whole = run(&mut channel(AptParams::default()), &iq, &[iq.len()]);
    let ragged = run(&mut channel(AptParams::default()), &iq, &BLOCKS);
    assert_eq!(whole.images.len(), 1);
    assert_eq!(ragged.images.len(), 1);
    assert_eq!(whole.images[0].picture, ragged.images[0].picture);
}

#[test]
fn doppler_and_noise_still_decode() {
    let mut iq = transmission(160, AvhrrChannel::Ch2, AvhrrChannel::Ch4, RATE);
    doppler(&mut iq, 3_000.0, 1_000.0, RATE);
    synth::add_noise(&mut iq, 0x5eed_1234, 0.3);
    let mut filtered = Vec::new();
    channel_filter(&AptParams::default())
        .expect("filter")
        .process(&iq, &mut filtered);
    filtered.extend_from_slice(&tail(10.0));
    let (image, event) = decode_one(&filtered);
    assert!(image.complete);
    let error = pixel_error(&image.picture);
    assert!(error < 6.0, "noisy mean error {error:.1}/255");
    assert_eq!(event.channel_a, Some(AvhrrChannel::Ch2));
    assert_eq!(event.channel_b, Some(AvhrrChannel::Ch4));
}

#[test]
fn a_slanted_clock_still_lands_in_frame() {
    let straight = transmission(160, AvhrrChannel::Ch2, AvhrrChannel::Ch4, RATE);
    let mut iq = synth::resample(&straight, RATE, RATE * 1.0005);
    iq.extend_from_slice(&tail(10.0));
    let (image, _) = decode_one(&iq);
    let error = pixel_error(&image.picture);
    assert!(error < 3.0, "slanted mean error {error:.1}/255");
}

#[test]
fn a_brief_fade_keeps_one_picture() {
    let mut iq = pass(160);
    let line = (RATE * 0.5) as usize;
    iq[line * 80..line * 86].fill(Complex::new(0.0, 0.0));
    let (image, _) = decode_one(&iq);
    assert!(image.complete);
    assert!(image.lines >= 157, "{} lines", image.lines);
}

#[test]
fn pure_noise_decodes_to_nothing() {
    for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
        let noise = complex_noise(seed, 0.4, (RATE * 20.0) as usize);
        let received = run(&mut channel(AptParams::default()), &noise, &BLOCKS);
        assert!(received.images.is_empty(), "seed {seed:#x} made a picture");
        assert_eq!(received.progress, 0);
    }
}

#[test]
fn a_pass_cut_short_is_kept_as_a_partial_picture() {
    let received = run(&mut channel(AptParams::default()), &pass(48), &BLOCKS);
    assert_eq!(received.images.len(), 1);
    let image = &received.images[0];
    assert!(!image.complete, "a short pass claimed to be complete");
    assert!((45..=48).contains(&image.lines), "{} lines", image.lines);
    assert!(!received.events[0].complete);
}

#[test]
fn dropping_partials_keeps_only_finished_pictures() {
    let mut chan = channel(AptParams {
        keep_partial: false,
    });
    let received = run(&mut chan, &pass(48), &BLOCKS);
    assert!(received.images.is_empty(), "a partial survived the setting");
}

#[test]
fn a_full_picture_ends_visibly_and_the_pass_continues() {
    let iq = pass(64);
    let mut chan = channel(AptParams::default());
    let mut out = ChannelOutputs::default();
    let quarter = iq.len() / 6;
    chan.process(&iq[..quarter], &mut out);
    assert!(chan.picture.active);
    chan.picture.lines = picture::MAX_LINES - 4;
    let mut received = run(&mut chan, &iq[quarter..], &BLOCKS);
    received.images.append(&mut out.images);
    assert_eq!(received.images.len(), 2);
    let capped = &received.images[0];
    assert!(!capped.complete);
    assert_eq!(usize::from(capped.lines), picture::MAX_LINES);
    assert!(received.images[1].lines > 16);
}

fn retune_midway(p: AptParams) -> (AptChannel, Received) {
    let iq = transmission(80, AvhrrChannel::Ch2, AvhrrChannel::Ch4, RATE);
    let mut chan = channel(p);
    let mut out = ChannelOutputs::default();
    chan.process(&iq[..iq.len() / 2], &mut out);
    assert!(chan.picture.active);
    chan.retuned();
    assert!(!chan.picture.active);
    let received = run(&mut chan, &tail(12.0), &BLOCKS);
    (chan, received)
}

#[test]
fn retuning_drops_the_picture_in_flight() {
    let (chan, received) = retune_midway(AptParams {
        keep_partial: false,
    });
    assert!(
        received.images.is_empty(),
        "a retuned channel still emitted"
    );
    assert!(!chan.picture.active);
}

#[test]
fn retuning_keeps_a_partial_when_asked() {
    let (_, received) = retune_midway(AptParams::default());
    assert_eq!(received.images.len(), 1);
    assert!(!received.images[0].complete);
}

#[test]
fn mismatched_params_variant_is_rejected() {
    let mut chan = channel(AptParams::default());
    let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
    assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    let built = AptChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Nfm(NfmParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}

#[test]
fn wrong_input_rate_is_rejected() {
    let built = AptChannel::new(
        ChannelCtx {
            input_rate: 48_000.0,
        },
        settings(ChannelParams::Apt(AptParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}
