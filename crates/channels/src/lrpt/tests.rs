use num_complex::Complex;
use sdrmm_wire::{ChannelParams, LrptImage, LrptMode, LrptParams, NfmParams};

use super::{
    frame::Deframer,
    image::{BLUE_APID, RED_APID, WIDTH},
    jpeg::{Block, MCU_PIXELS, MCUS_PER_PACKET, McuDecoder},
    link::{PairMap, VCDU_BYTES, VcduHeader, conv_code},
    *,
};
use crate::{
    VideoPicture,
    synth::{
        self,
        lrpt::{Cadu, DEFAULT_APIDS, QUALITY, Scene, cadus, encode_mcus, modulate, packets},
    },
    testutil::{at_snr, complex_noise, settings},
};

const RATE: f64 = INPUT_RATE_HZ;
const BLOCKS: [usize; 5] = [4_096, 1, 997, 65_536, 12_288];
const ROWS: u16 = 3;

fn channel(mode: LrptMode) -> LrptChannel {
    LrptChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Lrpt(LrptParams { mode })),
    )
    .expect("builds")
}

#[derive(Default)]
struct Received {
    images: Vec<DecodedImage>,
    events: Vec<LrptImage>,
    progress: usize,
}

fn run(chan: &mut LrptChannel, iq: &[Complex<f32>], lens: &[usize], received: &mut Received) {
    let mut out = ChannelOutputs::default();
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
        for event in out.events.drain(..) {
            match event {
                DecoderEvent::Lrpt(image) => received.events.push(image),
                other => panic!("unexpected event {other:?}"),
            }
        }
        at = end;
    }
}

fn with_tail(mut iq: Vec<Complex<f32>>) -> Vec<Complex<f32>> {
    iq.extend(synth::silence((2.5 * RATE) as usize));
    iq
}

fn decode(mode: LrptMode, iq: &[Complex<f32>], lens: &[usize]) -> Received {
    let mut chan = channel(mode);
    let mut received = Received::default();
    run(&mut chan, iq, lens, &mut received);
    received
}

fn single(received: Received) -> (DecodedImage, LrptImage) {
    assert_eq!(received.images.len(), 1, "expected one picture");
    assert_eq!(received.events.len(), 1, "expected one event");
    let image = received.images.into_iter().next().expect("one image");
    let event = received.events.into_iter().next().expect("one event");
    (image, event)
}

fn composite_error(scene: &Scene, got: &VideoPicture, rows: std::ops::Range<usize>) -> f64 {
    let red = scene.plane(RED_APID).expect("red channel");
    let blue = scene.plane(BLUE_APID).expect("blue channel");
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for index in rows.start * 8 * WIDTH..rows.end * 8 * WIDTH {
        let want = [red[index], red[index], blue[index]];
        for (channel, wanted) in want.into_iter().enumerate() {
            sum += f64::from(got.rgb[index * 3 + channel].abs_diff(wanted));
            count += 1;
        }
    }
    sum / count as f64
}

fn scene_cadus(apids: &[u16], rows: u16) -> (Scene, Vec<Cadu>) {
    let scene = Scene::new(apids, rows);
    let frames = cadus(&packets(&scene, QUALITY, 16_300));
    (scene, frames)
}

#[test]
fn every_mode_decodes_the_full_chain() {
    for mode in LrptMode::ALL {
        let (scene, frames) = scene_cadus(&DEFAULT_APIDS, ROWS);
        let iq = with_tail(modulate(mode, &frames, RATE));
        let received = decode(mode, &iq, &BLOCKS);
        assert!(received.progress > 1, "{mode:?} sent no progress");
        let (image, event) = single(received);
        assert_eq!(image.picture.width as usize, WIDTH);
        assert_eq!(image.lines, ROWS * 8, "{mode:?}");
        assert!(image.complete);
        assert_eq!(image.mode, "LRPT 221");
        assert_eq!(event.apids, DEFAULT_APIDS.to_vec());
        assert_eq!(event.packets_lost, 0, "{mode:?}");
        assert_eq!(event.frames_failed, 0, "{mode:?}");
        assert!(event.frames as usize >= frames.len() - 3, "{mode:?}");
        let error = composite_error(&scene, &image.picture, 0..usize::from(ROWS));
        assert!(error < 3.0, "{mode:?} mean error {error}");
    }
}

#[test]
fn ragged_blocks_give_the_same_picture() {
    let iq = with_tail(synth::lrpt::transmission(LrptMode::Oqpsk72, 2, RATE));
    let (whole, _) = single(decode(LrptMode::Oqpsk72, &iq, &[4_096]));
    let (ragged, _) = single(decode(LrptMode::Oqpsk72, &iq, &BLOCKS));
    assert_eq!(whole, ragged);
}

#[test]
fn noise_and_doppler_still_decode() {
    for (mode, doppler) in [
        (LrptMode::Oqpsk72, 5_000.0),
        (LrptMode::Qpsk72, -5_000.0),
        (LrptMode::Oqpsk80, 3_000.0),
    ] {
        let (scene, frames) = scene_cadus(&DEFAULT_APIDS, 2);
        let mut iq = modulate(mode, &frames, RATE);
        synth::shift(&mut iq, doppler, RATE);
        let iq = with_tail(at_snr(&iq, 3.0, 9));
        let (image, event) = single(decode(mode, &iq, &BLOCKS));
        assert_eq!(image.lines, 16, "{mode:?}");
        assert_eq!(event.packets_lost, 0, "{mode:?}");
        let error = composite_error(&scene, &image.picture, 0..2);
        assert!(error < 4.0, "{mode:?} mean error {error}");
    }
}

#[test]
fn damaged_frames_are_counted_and_leave_gaps_in_place() {
    let (scene, mut frames) = scene_cadus(&DEFAULT_APIDS, 4);
    for index in [100, 400, 700, 900] {
        frames[4][index] ^= 0x5A;
    }
    for byte in &mut frames[8][200..600] {
        *byte ^= 0xC3;
    }
    let iq = with_tail(modulate(LrptMode::Qpsk72, &frames, RATE));
    let (image, event) = single(decode(LrptMode::Qpsk72, &iq, &BLOCKS));
    assert_eq!(event.frames_failed, 1);
    assert!(event.frames_corrected >= 1);
    assert!(event.packets_lost > 0);
    assert_eq!(image.lines, 32);
    let first_row = composite_error(&scene, &image.picture, 0..1);
    let last_row = composite_error(&scene, &image.picture, 3..4);
    assert!(first_row < 3.0, "first row error {first_row}");
    assert!(last_row < 3.0, "last row error {last_row}");
}

#[test]
fn a_night_pass_shows_the_infrared_channel() {
    let (scene, frames) = scene_cadus(&[68], 2);
    let iq = with_tail(modulate(LrptMode::Oqpsk72, &frames, RATE));
    let (image, event) = single(decode(LrptMode::Oqpsk72, &iq, &BLOCKS));
    assert_eq!(image.mode, "LRPT APID 68");
    assert_eq!(event.apids, vec![68]);
    let plane = scene.plane(68).expect("plane");
    let error = plane
        .iter()
        .zip(&image.picture.luma)
        .map(|(&a, &b)| f64::from(a.abs_diff(b)))
        .sum::<f64>()
        / plane.len() as f64;
    assert!(error < 3.0, "mean error {error}");
}

#[test]
fn pure_noise_emits_nothing() {
    let noise = complex_noise(5, 0.5, (3.0 * RATE) as usize);
    let received = decode(LrptMode::Oqpsk72, &noise, &BLOCKS);
    assert!(received.images.is_empty());
    assert!(received.events.is_empty());
    assert_eq!(received.progress, 0);
}

#[test]
fn a_retune_closes_the_picture_as_partial() {
    let iq = synth::lrpt::transmission(LrptMode::Oqpsk72, 4, RATE);
    let mut chan = channel(LrptMode::Oqpsk72);
    let mut received = Received::default();
    run(&mut chan, &iq[..iq.len() * 3 / 5], &BLOCKS, &mut received);
    assert!(received.images.is_empty());
    chan.retuned();
    run(&mut chan, &synth::silence(4_096), &BLOCKS, &mut received);
    let (image, _) = single(received);
    assert!(!image.complete);
    assert!(image.lines > 0 && image.lines < 32);
}

#[test]
fn wrong_params_and_rates_are_rejected() {
    assert!(
        LrptChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Nfm(NfmParams::default())),
        )
        .is_err()
    );
    assert!(
        LrptChannel::new(
            ChannelCtx {
                input_rate: 48_000.0
            },
            settings(ChannelParams::Lrpt(LrptParams::default())),
        )
        .is_err()
    );
    let mut chan = channel(LrptMode::Qpsk72);
    assert!(
        chan.apply(settings(ChannelParams::Nfm(NfmParams::default())))
            .is_err()
    );
}

fn coded_soft(frames: &[Cadu]) -> Vec<i16> {
    let bits: Vec<bool> = frames
        .iter()
        .flat_map(|cadu| cadu.iter())
        .flat_map(|&byte| (0..8).rev().map(move |shift| byte >> shift & 1 == 1))
        .collect();
    let mut coded = Vec::new();
    conv_code().encode(&bits, &mut coded);
    coded
        .iter()
        .map(|&bit| if bit { 40 } else { -40 })
        .collect()
}

#[test]
fn the_deframer_resolves_every_rotation_and_offset() {
    let (_, frames) = scene_cadus(&DEFAULT_APIDS, 1);
    let soft = coded_soft(&frames);
    for map in PairMap::ALL {
        for skew in [0usize, 1] {
            let mut received: Vec<i16> = vec![25; skew];
            for &[first, second] in soft.as_chunks::<2>().0 {
                let (a, b) = map.apply(first, second);
                received.push(a);
                received.push(b);
            }
            let mut deframer = Deframer::new();
            let mut decoded = Vec::new();
            for chunk in received.chunks(3_000) {
                deframer.push(chunk);
                while let Some(outcome) = deframer.next_frame() {
                    decoded.push((outcome, VcduHeader::parse(deframer.vcdu())));
                }
            }
            assert_eq!(decoded.len(), frames.len() - 1, "{map:?} skew {skew}");
            assert!(decoded.iter().all(|(outcome, _)| !outcome.failed));
            assert!(
                decoded
                    .iter()
                    .any(|(_, header)| header.vcid == link::IMAGE_VCID)
            );
            assert_eq!(deframer.vcdu().len(), VCDU_BYTES);
        }
    }
}

#[test]
fn mcus_survive_the_jpeg_round_trip() {
    let blocks: Vec<Block> = (0..MCUS_PER_PACKET)
        .map(|index| {
            std::array::from_fn(|pixel| {
                let (y, x) = (pixel / 8, pixel % 8);
                (40 + index * 12 + x * 5 + y * 3) as u8
            })
        })
        .collect();
    let bytes = encode_mcus(&blocks, QUALITY);
    let mut decoder = McuDecoder::new(&bytes, QUALITY);
    let mut block = [0u8; MCU_PIXELS];
    for sent in &blocks {
        decoder.next_block(&mut block).expect("decodes");
        let error: u32 = sent
            .iter()
            .zip(&block)
            .map(|(&a, &b)| u32::from(a.abs_diff(b)))
            .sum();
        assert!(error / MCU_PIXELS as u32 <= 3, "mean error {error}");
    }
}
