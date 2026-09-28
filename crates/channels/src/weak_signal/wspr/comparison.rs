#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeSet, time::Instant};

use mfsk_core::wspr::{
    decode::{WsprCallsignTable, decode_scan_with_table},
    search::SearchParams,
};

use super::{SAMPLE_RATE, SLOT_SAMPLES, WsprDecoder, waveform};

struct Noise(u64);

impl Noise {
    fn gaussian(&mut self) -> f32 {
        let mut total = 0.0;
        for _ in 0..12 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            total += (self.0 >> 40) as f32 / (1u64 << 24) as f32;
        }
        total - 6.0
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % limit
    }
}

const CALLS: [&str; 8] = [
    "K1ABC", "W9XYZ", "G4ABC", "JA1XYZ", "DL1ABC", "VK2DEF", "PY2GHI", "ZS6JKL",
];
const GRIDS: [&str; 4] = ["FN42", "EN37", "JO22", "QF56"];
const POWERS: [&str; 4] = ["23", "30", "37", "10"];

fn slot(snr_db: f32, noise: &mut Noise, messages: &[String]) -> Vec<f32> {
    let noise_in_2500 = 2_500.0 / 6_000.0;
    let amplitude = (2.0 * noise_in_2500 * 10f32.powf(snr_db / 10.0)).sqrt();
    let mut audio: Vec<f32> = (0..SLOT_SAMPLES).map(|_| noise.gaussian()).collect();
    for (index, message) in messages.iter().enumerate() {
        let frequency = 1_420.0 + 40.0 * index as f32 + noise.below(100) as f32 * 0.07;
        let start = 12_000 + noise.below(24_000) as usize;
        for (sample, value) in audio[start..]
            .iter_mut()
            .zip(waveform(message, frequency).unwrap())
        {
            *sample += amplitude * value;
        }
    }
    audio.iter().map(|sample| sample / 40.0).collect()
}

fn reference(samples: &[f32], table: &mut WsprCallsignTable) -> BTreeSet<String> {
    let params = SearchParams {
        freq_min_hz: 1_400.0,
        freq_max_hz: 1_600.0,
        max_candidates: 200,
        ..SearchParams::default()
    };
    decode_scan_with_table(samples, SAMPLE_RATE as u32, 0, &params, table)
        .into_iter()
        .map(|result| result.message.to_string())
        .collect()
}

#[test]
#[ignore = "sensitivity sweep against mfsk-core; run in release"]
fn wspr_sweep_against_mfsk_core() {
    let mut noise = Noise(0xbeef);
    let mut decoder = WsprDecoder::new();
    let mut table = WsprCallsignTable::new();
    let slots: usize = std::env::var("SLOTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let snrs: Vec<f32> = std::env::var("SNRS")
        .map(|list| {
            list.split(',')
                .filter_map(|value| value.parse().ok())
                .collect()
        })
        .unwrap_or_else(|_| vec![-32.0, -31.0, -30.0, -29.0, -28.0, -27.0]);
    for snr in snrs {
        let (mut ours, mut theirs, mut total) = (0, 0, 0);
        let (mut ours_time, mut theirs_time) = (0.0, 0.0);
        for _ in 0..slots {
            let messages: Vec<String> = (0..4)
                .map(|_| {
                    format!(
                        "{} {} {}",
                        CALLS[noise.below(8) as usize],
                        GRIDS[noise.below(4) as usize],
                        POWERS[noise.below(4) as usize]
                    )
                })
                .collect();
            let audio = slot(snr, &mut noise, &messages);
            let started = Instant::now();
            let found: BTreeSet<String> = decoder
                .decode(&audio, 1_400.0, 1_600.0, 200)
                .into_iter()
                .map(|spot| spot.message.text)
                .collect();
            ours_time += started.elapsed().as_secs_f64();
            let started = Instant::now();
            let reference = reference(&audio, &mut table);
            theirs_time += started.elapsed().as_secs_f64();
            total += messages.len();
            ours += messages.iter().filter(|m| found.contains(*m)).count();
            theirs += messages.iter().filter(|m| reference.contains(*m)).count();
            let extra: Vec<_> = found.iter().filter(|m| !messages.contains(m)).collect();
            if !extra.is_empty() {
                println!("  extra {extra:?}");
            }
        }
        println!(
            "snr {snr:5.1}: ours {ours:3}/{total} {ours_time:.2}s | mfsk {theirs:3}/{total} {theirs_time:.2}s"
        );
    }
}

#[test]
#[ignore = "false decodes on pure noise; run in release"]
fn wspr_noise_false_decodes() {
    let mut noise = Noise(0x5eed);
    let mut decoder = WsprDecoder::new();
    let mut table = WsprCallsignTable::new();
    let slots: usize = std::env::var("SLOTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    for _ in 0..slots {
        let audio = slot(0.0, &mut noise, &[]);
        ours.extend(
            decoder
                .decode(&audio, 1_400.0, 1_600.0, 200)
                .into_iter()
                .map(|spot| spot.message.text),
        );
        theirs.extend(reference(&audio, &mut table));
    }
    println!(
        "{} false decodes in {slots} noise slots: {ours:?}",
        ours.len()
    );
    println!("mfsk-core: {} {theirs:?}", theirs.len());
}

#[test]
#[ignore = "crowded band against mfsk-core; run in release"]
fn wspr_crowded_band_against_mfsk_core() {
    let mut noise = Noise(0xc0ffee);
    let mut decoder = WsprDecoder::new();
    let mut table = WsprCallsignTable::new();
    let (mut ours, mut theirs, mut total) = (0, 0, 0);
    let (mut ours_time, mut theirs_time) = (0.0, 0.0);
    for _ in 0..8 {
        let mut audio: Vec<f32> = (0..SLOT_SAMPLES).map(|_| noise.gaussian()).collect();
        let mut messages = Vec::new();
        let mut placed = Vec::new();
        let count: usize = std::env::var("CROWD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(25);
        for index in 0..count {
            let text = format!(
                "{} {} {}",
                CALLS[noise.below(8) as usize],
                GRIDS[noise.below(4) as usize],
                POWERS[noise.below(4) as usize]
            );
            if messages.contains(&text) {
                continue;
            }
            let snr = -30.0 + noise.below(15) as f32;
            let amplitude = (2.0 * 2_500.0 / 6_000.0 * 10f32.powf(snr / 10.0)).sqrt();
            let frequency =
                1_405.0 + 190.0 / count as f32 * index as f32 + noise.below(20) as f32 * 0.1;
            let start = 12_000 + noise.below(24_000) as usize;
            for (sample, value) in audio[start..]
                .iter_mut()
                .zip(waveform(&text, frequency).unwrap())
            {
                *sample += amplitude * value;
            }
            messages.push(text);
            placed.push((frequency, snr, start));
        }
        let audio: Vec<f32> = audio.iter().map(|sample| sample / 40.0).collect();
        let started = Instant::now();
        let found: BTreeSet<String> = decoder
            .decode(&audio, 1_400.0, 1_600.0, 200)
            .into_iter()
            .map(|spot| spot.message.text)
            .collect();
        ours_time += started.elapsed().as_secs_f64();
        let started = Instant::now();
        let reference = reference(&audio, &mut table);
        theirs_time += started.elapsed().as_secs_f64();
        total += messages.len();
        ours += messages.iter().filter(|m| found.contains(*m)).count();
        theirs += messages.iter().filter(|m| reference.contains(*m)).count();
        let extra: Vec<_> = found.iter().filter(|m| !messages.contains(m)).collect();
        if !extra.is_empty() {
            println!("  extra {extra:?}");
        }
        for (index, message) in messages.iter().enumerate() {
            if !found.contains(message) {
                let (frequency, snr, start) = placed[index];
                let neighbours: Vec<_> = placed
                    .iter()
                    .filter(|p| (p.0 - frequency).abs() < 12.0 && p.0 != frequency)
                    .map(|p| (p.0 - frequency, p.1))
                    .collect();
                println!(
                    "  missed {message} {frequency:.1} Hz {snr} dB start {start} neighbours {neighbours:?} mfsk {}",
                    reference.contains(message)
                );
            }
        }
    }
    println!(
        "crowded: ours {ours}/{total} {ours_time:.2}s | mfsk {theirs}/{total} {theirs_time:.2}s"
    );
}
