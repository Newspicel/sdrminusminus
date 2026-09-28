#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Instant,
};

use mfsk_core::{
    ft8::Ft8,
    msg::{decode_request::DecodeRequest, wsjt77::unpack77},
};

use super::{FT8, FtxDecoder};

struct Recording {
    name: String,
    samples: Vec<f32>,
    published: BTreeSet<String>,
    snr: std::collections::BTreeMap<String, i32>,
}

fn unhashed(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.starts_with('<') && word.ends_with('>') {
                "<...>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn wav_samples(path: &Path) -> Option<Vec<f32>> {
    let bytes = std::fs::read(path).ok()?;
    let rate = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    if rate != 12_000 || u16::from_le_bytes([bytes[22], bytes[23]]) != 1 {
        return None;
    }
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        if &bytes[offset..offset + 4] == b"data" {
            let data = &bytes[offset + 8..(offset + 8 + size).min(bytes.len())];
            return Some(
                data.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
                    .collect(),
            );
        }
        offset += 8 + size;
    }
    None
}

fn recordings(dir: &Path, out: &mut Vec<Recording>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            recordings(&path, out);
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "wav") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path.with_extension("txt")) else {
            continue;
        };
        let Some(samples) = wav_samples(&path) else {
            continue;
        };
        let snr: std::collections::BTreeMap<String, i32> = text
            .lines()
            .filter_map(|line| {
                let (head, message) = line.split_once('~')?;
                let snr = head.split_whitespace().nth(1)?.parse().ok()?;
                Some((unhashed(message.trim().split("  ").next()?.trim()), snr))
            })
            .collect();
        out.push(Recording {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            samples,
            published: snr.keys().cloned().collect(),
            snr,
        });
    }
}

struct Score {
    hits: usize,
    extra: Vec<String>,
    missed: Vec<String>,
    seconds: f64,
}

fn score(recording: &Recording, decode: impl FnOnce(&[f32]) -> BTreeSet<String>) -> Score {
    let started = Instant::now();
    let texts: BTreeSet<String> = decode(&recording.samples)
        .iter()
        .map(|text| unhashed(text))
        .collect();
    let seconds = started.elapsed().as_secs_f64();
    Score {
        missed: recording
            .published
            .iter()
            .filter(|text| !texts.contains(*text))
            .map(|text| format!("{text} ({})", recording.snr[text]))
            .collect(),
        hits: recording
            .published
            .iter()
            .filter(|text| texts.contains(*text))
            .count(),
        extra: texts
            .into_iter()
            .filter(|text| !recording.published.contains(text))
            .collect(),
        seconds,
    }
}

fn reference(samples: &[f32]) -> BTreeSet<String> {
    let pcm: Vec<i16> = samples
        .iter()
        .map(|sample| (sample * 32_768.0).round() as i16)
        .collect();
    DecodeRequest::<Ft8>::new(&pcm, 200.0, 3_000.0, 1.0, 200)
        .sic_rounds(2)
        .decode()
        .results
        .iter()
        .filter_map(|result| unpack77(result.message77()))
        .collect()
}

#[test]
#[ignore = "needs FT8_CORPUS=<dir of WSJT-X referenced wav files>; run in release"]
fn ft8_corpus_against_mfsk_core() {
    let dir = PathBuf::from(std::env::var_os("FT8_CORPUS").expect("FT8_CORPUS"));
    let mut all = Vec::new();
    recordings(&dir, &mut all);
    let mut decoder = FtxDecoder::new(&FT8);
    let (mut published, mut ours_total, mut theirs_total) = (0, (0, 0, 0.0), (0, 0, 0.0));
    for recording in &all {
        let ours = score(recording, |samples| {
            decoder
                .decode(samples, 200.0, 3_000.0, 200)
                .into_iter()
                .map(|found| found.text)
                .collect()
        });
        let theirs = score(recording, reference);
        println!(
            "{:24} ref {:3} | ours {:3} +{:2} {:.3}s | mfsk {:3} +{:2} {:.3}s | {:?} | missed {:?} | mfsk missed {:?}",
            recording.name,
            recording.published.len(),
            ours.hits,
            ours.extra.len(),
            ours.seconds,
            theirs.hits,
            theirs.extra.len(),
            theirs.seconds,
            ours.extra,
            ours.missed,
            theirs.missed,
        );
        published += recording.published.len();
        ours_total = (
            ours_total.0 + ours.hits,
            ours_total.1 + ours.extra.len(),
            ours_total.2 + ours.seconds,
        );
        theirs_total = (
            theirs_total.0 + theirs.hits,
            theirs_total.1 + theirs.extra.len(),
            theirs_total.2 + theirs.seconds,
        );
    }
    println!(
        "TOTAL ref {published} | ours {} +{} {:.2}s | mfsk {} +{} {:.2}s",
        ours_total.0, ours_total.1, ours_total.2, theirs_total.0, theirs_total.1, theirs_total.2
    );
}

fn noise_false_decodes(protocol: &'static super::Protocol) {
    let mut decoder = FtxDecoder::new(protocol);
    let mut noise = Noise(0x1234_5678);
    let mut ours = Vec::new();
    let mut theirs = Vec::new();
    let slots: usize = std::env::var("SLOTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    for _ in 0..slots {
        let samples: Vec<f32> = (0..protocol.slot_samples)
            .map(|_| 0.1 * noise.gaussian())
            .collect();
        ours.extend(
            decoder
                .decode(&samples, 200.0, 3_000.0, 200)
                .into_iter()
                .map(|found| found.text),
        );
        theirs.extend(reference_decode(protocol, &samples));
    }
    println!(
        "{} false decodes in {slots} noise slots: {ours:?}",
        ours.len()
    );
    println!("mfsk-core: {} {theirs:?}", theirs.len());
}

#[test]
#[ignore = "false decodes on pure noise; run in release"]
fn ft8_noise_false_decodes() {
    noise_false_decodes(&FT8);
}

#[test]
#[ignore = "false decodes on pure noise; run in release"]
fn ft4_noise_false_decodes() {
    noise_false_decodes(&super::FT4);
}

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

fn slot_with_signals(
    protocol: &'static super::Protocol,
    snr_db: f32,
    noise: &mut Noise,
    messages: &[String],
) -> Vec<f32> {
    let sigma = 1.0f32;
    let noise_in_2500 = sigma * sigma * 2_500.0 / 6_000.0;
    let amplitude = (2.0 * noise_in_2500 * 10f32.powf(snr_db / 10.0)).sqrt();
    let mut audio: Vec<f32> = (0..protocol.slot_samples)
        .map(|_| sigma * noise.gaussian())
        .collect();
    for (index, message) in messages.iter().enumerate() {
        let payload = super::pack(message).unwrap();
        let frequency = 400.0 + 500.0 * index as f64 + noise.below(100) as f64 * 0.37;
        let start = 6_000 + noise.below(6_000) as usize;
        for (sample, value) in audio[start..]
            .iter_mut()
            .zip(super::waveform(protocol, payload, frequency))
        {
            *sample += amplitude * value;
        }
    }
    audio.iter().map(|sample| sample / 40.0).collect()
}

fn reference_decode(protocol: &'static super::Protocol, samples: &[f32]) -> BTreeSet<String> {
    let pcm: Vec<i16> = samples
        .iter()
        .map(|sample| (sample * 32_768.0).round() as i16)
        .collect();
    let results = if protocol.tones == 8 {
        DecodeRequest::<Ft8>::new(&pcm, 200.0, 3_000.0, 1.0, 200)
            .sic_rounds(2)
            .decode()
            .results
    } else {
        DecodeRequest::<mfsk_core::ft4::Ft4>::new(&pcm, 200.0, 3_000.0, 1.0, 200)
            .sic_rounds(2)
            .decode()
            .results
    };
    results
        .iter()
        .filter_map(|result| unpack77(result.message77()))
        .collect()
}

fn sweep(protocol: &'static super::Protocol, snrs: &[f32]) {
    let mut noise = Noise(0xfeed);
    let mut decoder = FtxDecoder::new(protocol);
    let slots = 20;
    for &snr in snrs {
        let (mut ours, mut theirs, mut total) = (0, 0, 0);
        let (mut ours_time, mut theirs_time) = (0.0, 0.0);
        for _ in 0..slots {
            let messages: Vec<String> = (0..5)
                .map(|_| {
                    format!(
                        "{} {} {}",
                        CALLS[noise.below(8) as usize],
                        CALLS[noise.below(8) as usize],
                        GRIDS[noise.below(4) as usize]
                    )
                })
                .collect();
            let audio = slot_with_signals(protocol, snr, &mut noise, &messages);
            let started = Instant::now();
            let found: BTreeSet<String> = decoder
                .decode(&audio, 200.0, 3_000.0, 200)
                .into_iter()
                .map(|found| found.text)
                .collect();
            ours_time += started.elapsed().as_secs_f64();
            let started = Instant::now();
            let reference = reference_decode(protocol, &audio);
            theirs_time += started.elapsed().as_secs_f64();
            total += messages.len();
            ours += messages.iter().filter(|m| found.contains(*m)).count();
            theirs += messages.iter().filter(|m| reference.contains(*m)).count();
        }
        println!(
            "snr {snr:5.1}: ours {ours:3}/{total} {ours_time:.2}s | mfsk {theirs:3}/{total} {theirs_time:.2}s"
        );
    }
}

#[test]
#[ignore = "sensitivity sweep against mfsk-core; run in release"]
fn ft8_sweep_against_mfsk_core() {
    sweep(&FT8, &[-25.0, -24.0, -23.0, -22.0, -21.0, -20.0]);
}

#[test]
#[ignore = "sensitivity sweep against mfsk-core; run in release"]
fn ft4_sweep_against_mfsk_core() {
    sweep(&super::FT4, &[-20.0, -19.0, -18.0, -17.0, -16.0, -15.0]);
}
