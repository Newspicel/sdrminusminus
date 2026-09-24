use num_complex::Complex;
use sdrmm_dsp::fft::FftPair;
use sdrmm_wire::{ChannelParams, DecoderEvent, IridiumParams, IridiumSpan};

use super::super::CHANNEL_RATE;
use super::super::modulate::modulate;
use super::super::sensitivity::{Found, Sent, es_n0_sigma, matches, traffic};
use super::super::tests::Gaussian;
use super::{USABLE_FRACTION, WidebandDecoder};
use crate::testutil::{run_events, settings};
use crate::{ChannelCtx, ChannelRx, IridiumChannel};

const RATE: f64 = 2_500_000.0;
const ES_N0_DB: f64 = 14.0;
const OFFSETS: [f64; 9] = [
    -950_000.0, -600_000.0, -300_000.0, 0.0, 41_667.0, 250_000.0, 520_000.0, 800_000.0, 960_000.0,
];

struct Placed {
    sent: Sent,
    offset_hz: f64,
}

fn upconvert(bits: &[u8], rate: f64, offset_hz: f64) -> Vec<Complex<f32>> {
    let len = modulate(bits, 64, CHANNEL_RATE, 0.0, 0.5).len();
    let decimation = (rate / CHANNEL_RATE).round() as usize;
    let bin_hz = CHANNEL_RATE / len as f64;
    let shift = (offset_hz / bin_hz).round() as i64;
    let mut narrow = modulate(
        bits,
        64,
        CHANNEL_RATE,
        offset_hz - shift as f64 * bin_hz,
        0.5,
    );
    FftPair::new(len).forward(&mut narrow);
    let wide_len = len * decimation;
    let mut wide = vec![Complex::default(); wide_len];
    for (i, value) in narrow.iter().enumerate() {
        let signed = if i < len / 2 {
            i as i64
        } else {
            i as i64 - len as i64
        };
        wide[(signed + shift).rem_euclid(wide_len as i64) as usize] = *value / len as f32;
    }
    FftPair::new(wide_len).inverse(&mut wide);
    wide
}

fn scene(
    rate: f64,
    offsets: &[f64],
    starts: &[f64],
    seconds: f64,
) -> (Vec<Placed>, Vec<Complex<f32>>) {
    let traffic = traffic(offsets.len(), 7);
    let mut iq = vec![Complex::default(); (seconds * rate) as usize];
    let mut placed = Vec::new();
    let mut power = 0.0;
    for (((sent, bits), &offset_hz), &start) in traffic.into_iter().zip(offsets).zip(starts) {
        let burst = upconvert(&bits, rate, offset_hz);
        power = burst.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / burst.len() as f64;
        let at = (start * rate) as usize;
        for (slot, sample) in iq[at..].iter_mut().zip(&burst) {
            *slot += sample;
        }
        placed.push(Placed { sent, offset_hz });
    }
    let sigma = es_n0_sigma(ES_N0_DB, power) * (rate / CHANNEL_RATE).sqrt() as f32;
    let mut noise = Gaussian(31);
    for sample in &mut iq {
        *sample += noise.sample(sigma);
    }
    (placed, iq)
}

fn decode(rate: f64, iq: &[Complex<f32>]) -> Vec<(Found, f32)> {
    let Ok(mut decoder) = WidebandDecoder::new(rate) else {
        return Vec::new();
    };
    let mut frames = Vec::new();
    for chunk in iq.chunks(65_536) {
        decoder.process(chunk, &mut frames);
    }
    frames
        .into_iter()
        .map(|f| {
            (
                (f.kind.to_owned(), f.details),
                f.offset_hz.unwrap_or(f32::NAN),
            )
        })
        .collect()
}

fn staggered(count: usize) -> Vec<f64> {
    (0..count)
        .map(|i| 0.08 + (i % 3) as f64 * 0.012 + (i / 3) as f64 * 0.16)
        .collect()
}

#[test]
fn decodes_bursts_across_the_span() {
    let (placed, iq) = scene(RATE, &OFFSETS, &staggered(OFFSETS.len()), 0.6);
    let found = decode(RATE, &iq);
    for burst in &placed {
        let hit = found.iter().find(|(f, _)| matches(&burst.sent, f));
        let Some((_, offset)) = hit else {
            panic!("missed the burst at {} Hz", burst.offset_hz);
        };
        assert!(
            (f64::from(*offset) - burst.offset_hz).abs() < 1_000.0,
            "burst at {} Hz reported at {offset} Hz",
            burst.offset_hz
        );
    }
}

#[test]
fn noise_alone_decodes_nothing() {
    let mut noise = Gaussian(77);
    let iq: Vec<Complex<f32>> = (0..(RATE as usize)).map(|_| noise.sample(0.3)).collect();
    assert!(decode(RATE, &iq).is_empty());
}

#[test]
fn a_wide_channel_reports_where_the_burst_was() {
    let params = IridiumParams {
        span: IridiumSpan::Mhz1,
    };
    let rate = crate::input_rate(&ChannelParams::Iridium(params));
    assert_eq!(rate, 1_000_000.0);
    assert_eq!(
        crate::occupied_band(&ChannelParams::Iridium(params)),
        (-rate * USABLE_FRACTION, rate * USABLE_FRACTION)
    );
    let (placed, iq) = scene(rate, &[-310_000.0], &[0.1], 0.2);
    let mut channel = IridiumChannel::new(
        ChannelCtx { input_rate: rate },
        settings(ChannelParams::Iridium(params)),
    )
    .expect("channel");
    let events = run_events(&mut channel, &iq);
    assert!(events.iter().any(|event| matches!(
        event,
        DecoderEvent::Iridium(message)
            if message.frequency_error_hz.is_some_and(|hz| (f64::from(hz) - placed[0].offset_hz).abs() < 1_000.0)
    )));
    assert!(
        IridiumChannel::new(
            ChannelCtx {
                input_rate: CHANNEL_RATE
            },
            settings(ChannelParams::Iridium(params)),
        )
        .is_err()
    );
}

#[test]
#[ignore = "measurement, run with --release"]
fn wideband_throughput() {
    let rate = 10_000_000.0;
    let count = 300;
    let mut state = 11u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let reach = rate * USABLE_FRACTION - 30_000.0;
    let offsets: Vec<f64> = (0..count).map(|_| (next() * 2.0 - 1.0) * reach).collect();
    let starts: Vec<f64> = (0..count).map(|_| 0.05 + next() * 0.9).collect();
    let (placed, iq) = scene(rate, &offsets, &starts, 1.0);
    let started = std::time::Instant::now();
    let found = decode(rate, &iq);
    let elapsed = started.elapsed().as_secs_f64();
    let hits = placed
        .iter()
        .filter(|p| found.iter().any(|(f, _)| matches(&p.sent, f)))
        .count();
    let mut noise = Gaussian(3);
    let quiet: Vec<Complex<f32>> = (0..rate as usize).map(|_| noise.sample(0.3)).collect();
    let started = std::time::Instant::now();
    let silent = decode(rate, &quiet).len();
    let idle = started.elapsed().as_secs_f64();
    eprintln!(
        "10 MHz, {count} bursts/s: {:.1}x realtime, {hits}/{count} decoded; noise only: {:.1}x realtime, {silent} frames",
        1.0 / elapsed,
        1.0 / idle
    );
}
