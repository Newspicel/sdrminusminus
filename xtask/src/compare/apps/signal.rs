use std::{
    f64::consts::TAU,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use num_complex::Complex;

pub const RATE: f64 = 10_000_000.0;
pub const CENTER_HZ: f64 = 100_000_000.0;
pub const STEM: &str = "compare_10msps";
pub const CARRIERS: usize = 16;

const SECONDS: usize = 4;
const SAMPLES: usize = RATE as usize * SECONDS;
const BLOCK: usize = 65_536;
const TONE_HZ: f64 = 1_000.0;
const DEVIATION_HZ: f64 = 2_500.0;
const CARRIER_LEVEL: f32 = 0.04;
const NOISE_LEVEL: f32 = 0.02;

pub struct Signal {
    pub dir: PathBuf,
    pub raw: PathBuf,
}

pub fn offset_hz(index: usize) -> f64 {
    -4_000_000.0 + index as f64 * 500_000.0
}

pub fn prepare(root: &Path) -> Result<Signal> {
    let dir = root.join("target/compare/apps/iq");
    let stem = dir.join(STEM);
    let signal = Signal {
        raw: sdrmm_recorder::data_path(&stem),
        dir,
    };
    if ready(&signal, &stem) {
        return Ok(signal);
    }
    std::fs::create_dir_all(&signal.dir)
        .with_context(|| format!("create {}", signal.dir.display()))?;
    for path in [sdrmm_recorder::meta_path(&stem), signal.raw.clone()] {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    println!("writing {SECONDS} s of {} MS/s IQ", RATE / 1e6);
    write(&stem)?;
    Ok(signal)
}

fn ready(signal: &Signal, stem: &Path) -> bool {
    let size = |path: &Path| std::fs::metadata(path).map(|meta| meta.len()).ok();
    sdrmm_recorder::meta_path(stem).exists() && size(&signal.raw) == Some(SAMPLES as u64 * 8)
}

fn write(stem: &Path) -> Result<()> {
    let mut sigmf = sdrmm_recorder::SigmfWriter::create(stem, RATE, CENTER_HZ, "SDR-- compare")
        .context("create the comparison recording")?;
    let mut source = Source::new();
    let mut block = vec![Complex::default(); BLOCK];
    let mut written = 0;
    while written < SAMPLES {
        let len = BLOCK.min(SAMPLES - written);
        source.fill(&mut block[..len]);
        sigmf
            .write_block(&block[..len])
            .context("write the comparison recording")?;
        written += len;
    }
    sigmf
        .finalize()
        .context("finalize the comparison recording")?;
    Ok(())
}

struct Source {
    modulation: Vec<Complex<f32>>,
    phasors: Vec<Complex<f32>>,
    steps: Vec<Complex<f32>>,
    index: usize,
    noise: u64,
}

impl Source {
    fn new() -> Self {
        let period = (RATE / TONE_HZ) as usize;
        let beta = DEVIATION_HZ / TONE_HZ;
        let modulation = (0..period)
            .map(|n| {
                Complex::from_polar(
                    CARRIER_LEVEL,
                    (beta * (TAU * n as f64 / period as f64).sin()) as f32,
                )
            })
            .collect();
        let steps = (0..CARRIERS)
            .map(|k| Complex::from_polar(1.0, (TAU * offset_hz(k) / RATE) as f32))
            .collect();
        Self {
            modulation,
            phasors: vec![Complex::new(1.0, 0.0); CARRIERS],
            steps,
            index: 0,
            noise: 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn fill(&mut self, block: &mut [Complex<f32>]) {
        for sample in block.iter_mut() {
            let tone = self.modulation[self.index % self.modulation.len()];
            let mut sum = Complex::new(self.uniform(), self.uniform()) * NOISE_LEVEL;
            for (phasor, step) in self.phasors.iter_mut().zip(&self.steps) {
                sum += tone * *phasor;
                *phasor *= step;
            }
            *sample = sum;
            self.index += 1;
        }
        for phasor in &mut self.phasors {
            *phasor /= phasor.norm();
        }
    }

    fn uniform(&mut self) -> f32 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 7;
        self.noise ^= self.noise << 17;
        (self.noise >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_carrier_sits_inside_the_band() {
        for k in 0..CARRIERS {
            assert!(offset_hz(k).abs() < RATE / 2.0 * 0.9);
        }
    }

    #[test]
    fn the_signal_stays_inside_full_scale() {
        let mut source = Source::new();
        let mut block = vec![Complex::default(); 4_096];
        source.fill(&mut block);
        assert!(block.iter().all(|s| s.re.abs() < 1.0 && s.im.abs() < 1.0));
        assert!(block.iter().any(|s| s.norm() > 0.1));
    }
}
