#![allow(clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use sdrmm_channels::{
    ChannelCtx, ChannelOutputs, ChannelRx, Dvbs2Rate,
    synth::{
        add_noise,
        adsb::{me_identification, squitter, transmission},
        datv::{Ldpc, LdpcFrame},
    },
};
use sdrmm_modem_test_support::ber::rng::Rng;
use sdrmm_wire::{AdsbParams, ChannelParams, ChannelSettings, Squelch};

const EB_N0_DB: f64 = 2.0;
const WORDS: usize = 4;

fn noisy_words(code: &Ldpc, rate: f64, seed: u64) -> Vec<Vec<f32>> {
    let mut rng = Rng::new(seed);
    let variance = 1.0 / (2.0 * rate * 10f64.powf(EB_N0_DB / 10.0));
    let sigma = variance.sqrt();
    let information = Dvbs2Rate::R1_2.information(LdpcFrame::Normal);
    (0..WORDS)
        .map(|_| {
            let message: Vec<bool> = (0..information).map(|_| rng.next_u64() & 1 == 1).collect();
            let mut codeword = Vec::new();
            code.encode(&message, &mut codeword);
            codeword
                .iter()
                .map(|&bit| {
                    let symbol = if bit { -1.0 } else { 1.0 };
                    (2.0 * (symbol + sigma * rng.normal()) / variance) as f32
                })
                .collect()
        })
        .collect()
}

fn ldpc_normal_half(c: &mut Criterion) {
    let mut code = Ldpc::new(Dvbs2Rate::R1_2, LdpcFrame::Normal).expect("normal 1/2");
    let words = noisy_words(&code, 0.5, 0x1d9c);
    let mut out = Vec::new();
    for word in &words {
        out.clear();
        code.decode(word, &mut out).expect("a converging word");
    }
    let mut group = c.benchmark_group("ldpc_normal_1_2");
    group.throughput(Throughput::Elements((WORDS * words[0].len()) as u64));
    group.bench_function("decode", |b| {
        b.iter(|| {
            for word in &words {
                out.clear();
                black_box(code.decode(black_box(word), &mut out));
            }
        });
    });
    group.finish();
}

const ADSB_RATE: f64 = 2_400_000.0;
const ADSB_SAMPLES: usize = 2_400_000;
const ADSB_BLOCK: usize = 65_536;

fn adsb_sky() -> Vec<Complex<f32>> {
    let frames: Vec<Vec<u8>> = (0..200)
        .map(|k| squitter(0x40_0000 + k, me_identification("SDRMM")))
        .collect();
    let mut iq = transmission(&frames, 5_000.0, 0.5, ADSB_RATE);
    iq.resize(ADSB_SAMPLES, Complex::default());
    add_noise(&mut iq, 0xad5b, 0.05);
    iq
}

fn adsb_decode(c: &mut Criterion) {
    let settings = ChannelSettings {
        frequency_hz: 0.0,
        squelch: Squelch::Off,
        params: ChannelParams::Adsb(AdsbParams::default()),
        blanker: Default::default(),
    };
    let ctx = ChannelCtx {
        input_rate: ADSB_RATE,
    };
    let iq = adsb_sky();
    let mut out = ChannelOutputs::default();
    let mut group = c.benchmark_group("adsb");
    group.throughput(Throughput::Elements(iq.len() as u64));
    group.sample_size(20);
    group.bench_function("one_second", |b| {
        b.iter(|| {
            let mut channel =
                sdrmm_channels::AdsbChannel::new(ctx, settings.clone()).expect("adsb channel");
            let mut decoded = 0;
            for block in iq.chunks(ADSB_BLOCK) {
                out.reset();
                channel.process(black_box(block), &mut out);
                decoded += out.events.len();
            }
            black_box(decoded)
        });
    });
    group.finish();
}

criterion_group!(benches, ldpc_normal_half, adsb_decode);
criterion_main!(benches);
