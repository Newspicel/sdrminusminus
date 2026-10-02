use num_complex::Complex;
use sdrmm_wire::{ChannelParams, NfmParams, WefaxIoc, WefaxLpm, WefaxParams, WefaxPicture};

use super::*;
use crate::{
    VideoPicture,
    synth::{
        self,
        wefax::{Chart, Plan, bars, transmission, transmission_with},
    },
    testutil::{complex_noise, settings},
};

const RATE: f64 = INPUT_RATE_HZ;
const BLOCKS: [usize; 6] = [4_096, 1, 997, 65_536, 33, 12_288];
const LINES: u16 = 64;
const SHORT: Plan = Plan {
    start_ms: 2_500.0,
    phasing_lines: 20,
    stop_ms: 2_500.0,
    black_ms: 500.0,
};

fn channel(p: WefaxParams) -> WefaxChannel {
    WefaxChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Wefax(p)),
    )
    .expect("builds")
}

fn fast() -> WefaxParams {
    WefaxParams {
        lpm: WefaxLpm::Lpm240,
        ..WefaxParams::default()
    }
}

struct Received {
    images: Vec<DecodedImage>,
    events: Vec<WefaxPicture>,
    progress: usize,
}

fn run(chan: &mut WefaxChannel, iq: &[Complex<f32>], lens: &[usize]) -> Received {
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
                DecoderEvent::Wefax(picture) => received.events.push(*picture),
                other => panic!("unexpected event {other:?}"),
            }
        }
        at = end;
    }
    received
}

fn tail(ms: f64) -> Vec<Complex<f32>> {
    synth::silence(samples(ms, RATE) as usize)
}

fn short(ioc: WefaxIoc, chart: &Chart) -> Vec<Complex<f32>> {
    let mut iq = transmission_with(ioc, WefaxLpm::Lpm240, chart, RATE, &SHORT);
    iq.extend_from_slice(&tail(1_000.0));
    iq
}

fn single(received: Received) -> DecodedImage {
    assert_eq!(received.images.len(), 1, "expected one picture");
    received.images.into_iter().next().expect("one image")
}

fn interior_error(sent: &Chart, got: &VideoPicture) -> f64 {
    assert_eq!(got.width, sent.width);
    let guard = u32::from(sent.width / 64).max(4);
    let edges: Vec<u32> = (1..8).map(|bar| u32::from(sent.width) * bar / 8).collect();
    let mut sum = 0.0f64;
    let mut count = 0u64;
    for y in 0..sent.height.min(got.height) {
        for x in 0..sent.width {
            if edges
                .iter()
                .any(|&edge| u32::from(x).abs_diff(edge) <= guard)
            {
                continue;
            }
            let at = usize::from(y) * usize::from(sent.width) + usize::from(x);
            sum += f64::from(got.luma[at].abs_diff(sent.pixel(x, y)));
            count += 1;
        }
    }
    sum / count as f64
}

fn marked(ioc: WefaxIoc, lines: u16, column: u16) -> Chart {
    let mut chart = Chart::new(ioc.width(), lines);
    chart.levels.fill(255);
    for y in 0..lines {
        for x in column..column + 6 {
            chart.set(x, y, 0);
        }
    }
    chart
}

fn darkest_column(picture: &VideoPicture, row: u16) -> u16 {
    let width = usize::from(picture.width);
    let line = &picture.luma[usize::from(row) * width..][..width];
    let mut best = 0;
    for x in 0..width.saturating_sub(6) {
        let dark: u32 = line[x..x + 6].iter().map(|&level| u32::from(level)).sum();
        let top: u32 = line[best..best + 6]
            .iter()
            .map(|&level| u32::from(level))
            .sum();
        if dark < top {
            best = x;
        }
    }
    best as u16
}

#[test]
fn the_widest_picture_fits_the_store() {
    assert_eq!(usize::from(WefaxIoc::Ioc576.width()), picture::MAX_WIDTH);
}

#[test]
fn the_start_tone_picks_the_index_of_cooperation() {
    for ioc in WefaxIoc::ALL {
        let sent = bars(ioc, LINES);
        let mut chan = channel(WefaxParams {
            ioc: WefaxIoc::Ioc576,
            ..fast()
        });
        let image = single(run(&mut chan, &short(ioc, &sent), &BLOCKS));
        assert_eq!(image.picture.width, ioc.width(), "{ioc:?}");
        assert_eq!(image.mode, format!("IOC {} · 240 LPM", ioc.value()));
    }
}

#[test]
fn phasing_alone_uses_the_configured_index() {
    let ioc = WefaxIoc::Ioc288;
    let sent = bars(ioc, LINES);
    let plan = Plan {
        start_ms: 0.0,
        ..SHORT
    };
    let mut iq = transmission_with(ioc, WefaxLpm::Lpm240, &sent, RATE, &plan);
    iq.extend_from_slice(&tail(1_000.0));
    let mut chan = channel(WefaxParams { ioc, ..fast() });
    let image = single(run(&mut chan, &iq, &BLOCKS));
    assert!(image.complete);
    assert_eq!(image.lines, LINES);
    let error = interior_error(&sent, &image.picture);
    assert!(error < 8.0, "mean interior error {error:.1}/255");
}

#[test]
fn every_index_round_trips() {
    for ioc in WefaxIoc::ALL {
        let sent = bars(ioc, LINES);
        let mut chan = channel(fast());
        let received = run(&mut chan, &short(ioc, &sent), &BLOCKS);
        assert!(received.progress > 1, "{ioc:?} sent no progressive updates");
        let image = single(received);
        assert!(image.complete, "{ioc:?} did not complete");
        assert_eq!(image.source, SOURCE);
        assert_eq!(image.lines, LINES, "{ioc:?}");
        let error = interior_error(&sent, &image.picture);
        assert!(error < 8.0, "{ioc:?} mean interior error {error:.1}/255");
    }
}

#[test]
fn a_standard_transmission_at_120_lpm_decodes() {
    let ioc = WefaxIoc::Ioc576;
    let sent = bars(ioc, 40);
    let iq = transmission(ioc, WefaxLpm::Lpm120, &sent, RATE);
    let received = run(&mut channel(WefaxParams::default()), &iq, &BLOCKS);
    assert_eq!(received.events.len(), 1);
    let event = received.events[0];
    assert!(event.complete);
    assert_eq!((event.width, event.lines), (1_810, 40));
    assert_eq!(
        (event.ioc, event.lpm, event.seq),
        (ioc, WefaxLpm::Lpm120, 1)
    );
    let expected = 5_000.0 + 30_000.0 + 40.0 * 500.0;
    let actual = f64::from(event.duration_ms);
    assert!(
        (actual - expected).abs() < 500.0,
        "reported {actual} ms against {expected} ms"
    );
    let error = interior_error(&sent, &single(received).picture);
    assert!(error < 6.0, "mean interior error {error:.1}/255");
}

#[test]
fn phasing_finds_the_line_start() {
    let ioc = WefaxIoc::Ioc576;
    let column = 900;
    let sent = marked(ioc, LINES, column);
    let image = single(run(&mut channel(fast()), &short(ioc, &sent), &BLOCKS));
    for row in 0..image.lines {
        let found = darkest_column(&image.picture, row);
        assert!(
            found.abs_diff(column) <= 3,
            "row {row} marker at {found}, sent at {column}"
        );
    }
}

#[test]
fn phasing_corrects_a_slanted_clock() {
    let ioc = WefaxIoc::Ioc576;
    let column = 900;
    let sent = marked(ioc, 120, column);
    let straight = transmission_with(ioc, WefaxLpm::Lpm240, &sent, RATE, &SHORT);
    let mut iq = synth::resample(&straight, RATE, RATE * 1.0003);
    iq.extend_from_slice(&tail(1_000.0));
    let image = single(run(&mut channel(fast()), &iq, &BLOCKS));
    assert!(image.complete);
    for row in [0, 60, image.lines - 1] {
        let found = darkest_column(&image.picture, row);
        assert!(
            found.abs_diff(column) <= 3,
            "row {row} marker at {found}, sent at {column}"
        );
    }
}

#[test]
fn a_transmission_cut_short_is_kept_as_a_partial_picture() {
    let ioc = WefaxIoc::Ioc576;
    let sent = bars(ioc, 120);
    let full = transmission_with(ioc, WefaxLpm::Lpm240, &sent, RATE, &SHORT);
    let picture_start = samples(SHORT.start_ms + 20.0 * 250.0, RATE) as usize;
    let cut = picture_start + samples(60.0 * 250.0, RATE) as usize;
    let mut iq = full[..cut].to_vec();
    iq.extend_from_slice(&tail(5_000.0));

    let received = run(&mut channel(fast()), &iq, &BLOCKS);
    let image = single(received);
    assert!(
        !image.complete,
        "a truncated picture claimed to be complete"
    );
    assert!(
        (58..=61).contains(&image.lines),
        "kept {} lines of 60",
        image.lines
    );
    let error = interior_error(&sent, &image.picture);
    assert!(error < 8.0, "mean interior error {error:.1}/255");
}

#[test]
fn dropping_partials_keeps_only_finished_pictures() {
    let ioc = WefaxIoc::Ioc576;
    let full = short(ioc, &bars(ioc, 120));
    let mut iq = full[..full.len() / 2].to_vec();
    iq.extend_from_slice(&tail(5_000.0));
    let mut chan = channel(WefaxParams {
        keep_partial: false,
        ..fast()
    });
    let received = run(&mut chan, &iq, &BLOCKS);
    assert!(received.images.is_empty(), "a partial survived the setting");
    assert!(received.events.is_empty());
}

#[test]
fn decodes_through_additive_noise() {
    let ioc = WefaxIoc::Ioc576;
    let sent = bars(ioc, LINES);
    let mut iq = short(ioc, &sent);
    synth::add_noise(&mut iq, 0xabad_1dea, 0.25);
    let mut filtered = Vec::new();
    channel_filter(&fast())
        .expect("filter")
        .process(&iq, &mut filtered);
    let image = single(run(&mut channel(fast()), &filtered, &BLOCKS));
    assert!(image.complete);
    assert_eq!(image.lines, LINES);
    let error = interior_error(&sent, &image.picture);
    assert!(error < 15.0, "noisy mean interior error {error:.1}/255");
}

#[test]
fn pure_noise_decodes_to_nothing() {
    for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
        let noise = complex_noise(seed, 0.4, 600_000);
        let mut filtered = Vec::new();
        channel_filter(&fast())
            .expect("filter")
            .process(&noise, &mut filtered);
        for iq in [&noise, &filtered] {
            let mut chan = channel(fast());
            let received = run(&mut chan, iq, &BLOCKS);
            assert!(
                received.images.is_empty(),
                "seed {seed:#x} produced {} images",
                received.images.len()
            );
            assert!(!chan.picture.active, "seed {seed:#x} left a picture open");
        }
    }
}

#[test]
fn ragged_block_splits_decode_identically() {
    let ioc = WefaxIoc::Ioc288;
    let iq = short(ioc, &bars(ioc, LINES));
    let whole = run(&mut channel(fast()), &iq, &[iq.len()]);
    let ragged = run(&mut channel(fast()), &iq, &BLOCKS);
    assert_eq!(whole.images.len(), 1);
    assert_eq!(ragged.images.len(), 1);
    assert_eq!(whole.images[0], ragged.images[0]);
}

#[test]
fn retuning_drops_the_picture_in_flight() {
    let ioc = WefaxIoc::Ioc576;
    let iq = short(ioc, &bars(ioc, LINES));
    let split = samples(SHORT.start_ms + 20.0 * 250.0 + 30.0 * 250.0, RATE) as usize;
    let mut chan = channel(fast());
    let mut out = ChannelOutputs::default();
    chan.process(&iq[..split], &mut out);
    assert!(chan.picture.active);
    chan.retuned();
    assert!(!chan.picture.active);
    out.reset();
    chan.process(&iq[split..], &mut out);
    assert!(out.images.is_empty(), "a retuned channel still emitted");
}

#[test]
fn changing_the_line_rate_abandons_the_picture() {
    let ioc = WefaxIoc::Ioc576;
    let iq = short(ioc, &bars(ioc, LINES));
    let split = samples(SHORT.start_ms + 20.0 * 250.0 + 30.0 * 250.0, RATE) as usize;
    let mut chan = channel(fast());
    let mut out = ChannelOutputs::default();
    chan.process(&iq[..split], &mut out);
    assert!(chan.picture.active);
    chan.apply(settings(ChannelParams::Wefax(WefaxParams::default())))
        .expect("applies");
    assert!(!chan.picture.active);
}

#[test]
fn a_second_transmission_follows_the_first() {
    let ioc = WefaxIoc::Ioc576;
    let sent = bars(ioc, LINES);
    let mut iq = short(ioc, &sent);
    iq.extend(short(WefaxIoc::Ioc288, &bars(WefaxIoc::Ioc288, LINES)));
    let received = run(&mut channel(fast()), &iq, &BLOCKS);
    assert_eq!(received.images.len(), 2);
    assert!(received.images.iter().all(|image| image.complete));
    assert_eq!(received.events[0].seq, 1);
    assert_eq!(received.events[1].seq, 2);
    assert_eq!(received.events[0].ioc, WefaxIoc::Ioc576);
    assert_eq!(received.events[1].ioc, WefaxIoc::Ioc288);
}

#[test]
fn mismatched_params_variant_is_rejected() {
    let mut chan = channel(WefaxParams::default());
    let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
    assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
    let built = WefaxChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Nfm(NfmParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}

#[test]
fn wrong_input_rate_is_rejected() {
    let built = WefaxChannel::new(
        ChannelCtx {
            input_rate: 48_000.0,
        },
        settings(ChannelParams::Wefax(WefaxParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}

#[test]
fn a_start_tone_without_phasing_still_starts_a_picture() {
    let ioc = WefaxIoc::Ioc576;
    let sent = bars(ioc, LINES);
    let plan = Plan {
        phasing_lines: 0,
        ..SHORT
    };
    let mut iq = transmission_with(ioc, WefaxLpm::Lpm240, &sent, RATE, &plan);
    iq.extend_from_slice(&tail(1_000.0));
    let image = single(run(&mut channel(fast()), &iq, &BLOCKS));
    assert!(image.complete);
    assert!(
        (LINES - 1..=LINES + 1).contains(&image.lines),
        "decoded {} lines of {LINES}",
        image.lines
    );
}
