#![allow(clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use sdrmm_channels::{
    Dvbs2Rate,
    synth::datv::{Ldpc, LdpcFrame},
};
use sdrmm_modem_test_support::ber::rng::Rng;

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

criterion_group!(benches, ldpc_normal_half);
criterion_main!(benches);
