use num_complex::Complex;
use serde_json::Value;

use super::decode::{ChannelDecoder, WidebandDecoder};
use super::encode::{da_burst_bits, ims_bits, ira_bits, ira_payload, pager_blocks};
use super::modulate::modulate;
use super::tests::{Gaussian, place};
use super::{CHANNEL_RATE, channel_filter};

#[derive(Clone)]
enum Sent {
    Ring { sat: u32, tmsi: String },
    Data { hex: String },
    Page { ric: u32 },
}

type Found = (String, Value);

fn traffic(count: usize, seed: u64) -> Vec<(Sent, Vec<u8>)> {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as u32
    };
    (0..count)
        .map(|i| match i % 3 {
            0 => {
                let sat = 1 + next() % 120;
                let tmsi = next();
                let bits = ira_bits(&ira_payload(sat, 5, [-700, 900, 1300], &[tmsi]));
                (
                    Sent::Ring {
                        sat,
                        tmsi: format!("{tmsi:08x}"),
                    },
                    bits,
                )
            }
            1 => {
                let payload: [u8; 20] = std::array::from_fn(|_| (next() & 0xff) as u8);
                let bits = da_burst_bits(false, 0, 20, &payload);
                (
                    Sent::Data {
                        hex: crate::datalink::hex(&payload),
                    },
                    bits,
                )
            }
            _ => {
                let ric = next() % 4_000_000;
                (
                    Sent::Page { ric },
                    ims_bits(&pager_blocks(ric, "WX 12KT")),
                )
            }
        })
        .collect()
}

fn matches(sent: &Sent, found: &Found) -> bool {
    let (kind, d) = found;
    match sent {
        Sent::Ring { sat, tmsi } => {
            kind == "ring-alert" && d["sat"] == *sat && d["pages"][0]["tmsi"] == tmsi.as_str()
        }
        Sent::Data { hex } => kind == "ida" && d["crc_ok"] == true && d["data_hex"] == hex.as_str(),
        Sent::Page { ric } => kind == "msg" && d["body"]["ric"] == *ric,
    }
}

fn filtered(iq: &[Complex<f32>]) -> Vec<Complex<f32>> {
    let mut filter = channel_filter();
    let mut out = Vec::new();
    let mut all = Vec::with_capacity(iq.len());
    for chunk in iq.chunks(8_192) {
        filter.process(chunk, &mut out);
        all.extend_from_slice(&out);
    }
    all
}

fn run_ours(iq: &[Complex<f32>]) -> Vec<Found> {
    let mut decoder = ChannelDecoder::new();
    let mut frames = Vec::new();
    for chunk in iq.chunks(8_192) {
        decoder.process(chunk, &mut frames);
    }
    frames
        .into_iter()
        .map(|f| (f.kind.to_owned(), f.details))
        .collect()
}

fn run_xng(iq: &[Complex<f32>]) -> Vec<Found> {
    let Ok(mut decoder) = xng_mode_iridium::IridiumChannelDecoder::new(CHANNEL_RATE, 0.0) else {
        return Vec::new();
    };
    iq.chunks(8_192)
        .flat_map(|chunk| decoder.process(chunk))
        .map(|f| (f.kind.to_owned(), f.details))
        .collect()
}

fn run_wideband(iq: &[Complex<f32>]) -> Vec<Found> {
    let Ok(mut decoder) = WidebandDecoder::new(CHANNEL_RATE) else {
        return Vec::new();
    };
    let mut frames = Vec::new();
    for chunk in iq.chunks(8_192) {
        decoder.process(chunk, &mut frames);
    }
    frames
        .into_iter()
        .map(|(_, f)| (f.kind.to_owned(), f.details))
        .collect()
}

fn score(sent: &[Sent], found: &[Found]) -> (usize, usize) {
    let hits = sent
        .iter()
        .filter(|s| found.iter().any(|f| matches(s, f)))
        .count();
    let decodable = |f: &&Found| matches!(f.0.as_str(), "ring-alert" | "ida" | "msg");
    let false_frames = found
        .iter()
        .filter(decodable)
        .filter(|f| !sent.iter().any(|s| matches(s, f)))
        .count();
    (hits, false_frames)
}

pub fn es_n0_sigma(es_n0_db: f64, amplitude_power: f64) -> f32 {
    let sps = CHANNEL_RATE / 25_000.0;
    let snr = 10f64.powf(es_n0_db / 10.0) / sps;
    (amplitude_power / snr / 2.0).sqrt() as f32
}

type Runner = fn(&[Complex<f32>]) -> Vec<Found>;

#[test]
#[ignore = "measurement, prints a table"]
fn sensitivity_sweep() {
    let runners: [(&str, Runner); 3] = [
        ("xng", run_xng),
        ("ours", run_ours),
        ("wideband", run_wideband),
    ];
    let bursts = traffic(90, 1);
    let sent: Vec<Sent> = bursts.iter().map(|(s, _)| s.clone()).collect();
    let iq: Vec<Vec<Complex<f32>>> = bursts
        .iter()
        .enumerate()
        .map(|(i, (_, bits))| {
            let cfo = (i as f64 * 2_917.0) % 16_000.0 - 8_000.0;
            modulate(bits, 64, CHANNEL_RATE, cfo, 0.5)
        })
        .collect();
    let power = iq
        .iter()
        .flat_map(|b| b.iter().skip(200).take(b.len().saturating_sub(400)))
        .map(|s| f64::from(s.norm_sqr()))
        .sum::<f64>()
        / iq.iter().map(|b| b.len().saturating_sub(400)).sum::<usize>() as f64;
    let clean = place(&iq, 25_000);
    eprintln!("Es/N0 dB | {}", runners.map(|(n, _)| n).join(" | "));
    for es_n0 in [20.0, 16.0, 13.0, 11.0, 10.0, 9.0, 8.0, 7.0, 6.0, 5.0, 4.0] {
        let sigma = es_n0_sigma(es_n0, power);
        let mut noise = Gaussian(es_n0 as u64 + 99);
        let noisy: Vec<Complex<f32>> = clean.iter().map(|&s| s + noise.sample(sigma)).collect();
        let input = filtered(&noisy);
        let row: Vec<String> = runners
            .iter()
            .map(|(_, run)| {
                let (hits, false_frames) = score(&sent, &run(&input));
                format!("{hits}/{} ({false_frames} false)", sent.len())
            })
            .collect();
        eprintln!("{es_n0:>5.1} | {}", row.join(" | "));
    }
    let mut noise_only = vec![Complex::default(); 2_500_000];
    let mut noise = Gaussian(5);
    for s in &mut noise_only {
        *s = noise.sample(0.05);
    }
    let input = filtered(&noise_only);
    for (name, run) in runners {
        let found = run(&input);
        eprintln!("noise only, {name}: {} frames", found.len());
    }
}
