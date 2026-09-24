use num_complex::Complex;
use serde_json::Value;

use super::encode::{da_burst_bits, ims_bits, ira_bits, ira_payload, pager_blocks};
use super::modulate::modulate;
use super::receiver::ChannelDecoder;
use super::tests::{Gaussian, place};
use super::{CHANNEL_RATE, channel_filter};

#[derive(Clone)]
pub(super) enum Sent {
    Ring { sat: u32, tmsi: String },
    Data { hex: String },
    Page { ric: u32 },
}

pub(super) type Found = (String, Value);
type Runner = fn(&[Complex<f32>]) -> Vec<Found>;

pub(super) fn traffic(count: usize, seed: u64) -> Vec<(Sent, Vec<u8>)> {
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
                let tmsi = format!("{tmsi:08x}");
                (Sent::Ring { sat, tmsi }, bits)
            }
            1 => {
                let payload: [u8; 20] = std::array::from_fn(|_| (next() & 0xff) as u8);
                let hex = crate::datalink::hex(&payload);
                (Sent::Data { hex }, da_burst_bits(false, 0, 20, &payload))
            }
            _ => {
                let ric = next() % 4_000_000;
                (Sent::Page { ric }, ims_bits(&pager_blocks(ric, "WX 12KT")))
            }
        })
        .collect()
}

pub(super) fn matches(sent: &Sent, found: &Found) -> bool {
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
    let mut filter = channel_filter(&sdrmm_wire::IridiumParams::default());
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

pub(super) fn es_n0_sigma(es_n0_db: f64, burst_power: f64) -> f32 {
    let sps = CHANNEL_RATE / 25_000.0;
    let snr = 10f64.powf(es_n0_db / 10.0) / sps;
    (burst_power / snr / 2.0).sqrt() as f32
}

struct Scene {
    sent: Vec<Sent>,
    clean: Vec<Complex<f32>>,
    power: f64,
}

fn scene(count: usize) -> Scene {
    let bursts = traffic(count, 1);
    let iq: Vec<Vec<Complex<f32>>> = bursts
        .iter()
        .enumerate()
        .map(|(i, (_, bits))| {
            let cfo = (i as f64 * 2_917.0) % 16_000.0 - 8_000.0;
            modulate(bits, 64, CHANNEL_RATE, cfo, 0.5)
        })
        .collect();
    let samples: usize = iq.iter().map(Vec::len).sum();
    let power = iq
        .iter()
        .flatten()
        .map(|s| f64::from(s.norm_sqr()))
        .sum::<f64>()
        / samples as f64;
    Scene {
        sent: bursts.into_iter().map(|(s, _)| s).collect(),
        clean: place(&iq, 25_000),
        power,
    }
}

fn noisy(scene: &Scene, es_n0: f64, seed: u64) -> Vec<Complex<f32>> {
    let sigma = es_n0_sigma(es_n0, scene.power);
    let mut noise = Gaussian(seed);
    let iq: Vec<Complex<f32>> = scene
        .clean
        .iter()
        .map(|&s| s + noise.sample(sigma))
        .collect();
    filtered(&iq)
}

fn hits(sent: &[Sent], found: &[Found]) -> usize {
    sent.iter()
        .filter(|s| found.iter().any(|f| matches(s, f)))
        .count()
}

#[test]
fn decodes_most_bursts_at_10_db() {
    let scene = scene(15);
    let found = run_ours(&noisy(&scene, 10.0, 109));
    assert!(hits(&scene.sent, &found) >= 9);
}

#[test]
#[ignore = "measurement, prints a table"]
fn sensitivity_sweep() {
    let runners: [(&str, Runner); 2] = [("xng", run_xng), ("ours", run_ours)];
    let scene = scene(90);
    eprintln!("Es/N0 dB | xng | ours");
    for es_n0 in [20.0, 16.0, 13.0, 11.0, 10.0, 9.0, 8.0, 7.0, 6.0] {
        let input = noisy(&scene, es_n0, es_n0 as u64 + 99);
        let row: Vec<String> = runners
            .iter()
            .map(|(_, run)| {
                let started = std::time::Instant::now();
                let found = run(&input);
                let elapsed = started.elapsed().as_secs_f64();
                format!(
                    "{}/{} in {elapsed:.2}s",
                    hits(&scene.sent, &found),
                    scene.sent.len()
                )
            })
            .collect();
        eprintln!("{es_n0:>5.1} | {}", row.join(" | "));
    }
    let mut noise = Gaussian(5);
    let noise_only: Vec<Complex<f32>> = (0..5_000_000).map(|_| noise.sample(0.05)).collect();
    let input = filtered(&noise_only);
    for (name, run) in runners {
        eprintln!("noise only, {name}: {} frames", run(&input).len());
    }
}
