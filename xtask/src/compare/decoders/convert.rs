use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow, ensure};
use num_complex::Complex;
use rustfft::FftPlanner;
use sdrmm_dsp::{Ddc, FmDemod, FracResampler};

const BLOCK: usize = 65_536;
const AUDIO_RATE: f64 = 48_000.0;
const AUDIO_GAIN: f32 = 0.5;

pub struct Fixture {
    pub iq: Vec<Complex<f32>>,
    pub rate: f64,
    zeros: Vec<Complex<f32>>,
}

#[derive(Clone, Copy)]
pub enum Format {
    Cu8 { rate: f64 },
    Cf32,
    Audio(Audio),
}

#[derive(Clone, Copy)]
pub struct Audio {
    pub demod: Demod,
    pub rate: u32,
    pub container: Container,
}

#[derive(Clone, Copy)]
pub enum Demod {
    Fm { deviation_hz: f64 },
    Am,
    Real,
}

#[derive(Clone, Copy)]
pub enum Container {
    Wav,
    Raw,
}

impl Fixture {
    pub fn load(root: &Path, stem: &str) -> Result<Self> {
        let base = root.join("fixtures").join(stem);
        let meta_path = base.with_extension("sigmf-meta");
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&meta_path).with_context(|| {
                format!("read {}; run `cargo xtask fixtures`", meta_path.display())
            })?)?;
        let global = &meta["global"];
        ensure!(
            global["core:datatype"] == "cf32_le",
            "{stem} is not cf32_le"
        );
        let rate = global["core:sample_rate"]
            .as_f64()
            .with_context(|| format!("{stem} has no sample rate"))?;
        let data_path = base.with_extension("sigmf-data");
        let bytes =
            std::fs::read(&data_path).with_context(|| format!("read {}", data_path.display()))?;
        let iq = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| {
                Complex::new(
                    f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    f32::from_le_bytes([s[4], s[5], s[6], s[7]]),
                )
            })
            .collect();
        Ok(Self {
            iq,
            rate,
            zeros: vec![Complex::default(); BLOCK],
        })
    }

    pub fn seconds(&self) -> f64 {
        self.iq.len() as f64 / self.rate
    }

    fn tail_samples(&self, tail_s: f64) -> usize {
        (tail_s * self.rate).round() as usize
    }

    pub fn feed(
        &self,
        loops: usize,
        tail_s: f64,
        mut each: impl FnMut(&[Complex<f32>]) -> Result<()>,
    ) -> Result<()> {
        for _ in 0..loops {
            for block in self.iq.chunks(BLOCK) {
                each(block)?;
            }
        }
        let mut left = self.tail_samples(tail_s);
        while left > 0 {
            let n = left.min(BLOCK);
            each(&self.zeros[..n])?;
            left -= n;
        }
        Ok(())
    }

    pub fn as_cu8(&self, rate: f64) -> Result<Self> {
        let length = self.iq.len() as f64 * rate / self.rate;
        ensure!(
            (length - length.round()).abs() < 1e-6,
            "{} Hz does not resample to a whole number of samples at {rate} Hz",
            self.rate
        );
        let iq = resample(&self.iq, length.round() as usize)
            .into_iter()
            .map(|s| Complex::new(dequantised(unsigned(s.re)), dequantised(unsigned(s.im))))
            .collect();
        Ok(Self {
            iq,
            rate,
            zeros: self.zeros.clone(),
        })
    }

    fn mean_magnitude(&self) -> f32 {
        self.iq.iter().map(|s| s.norm()).sum::<f32>() / self.iq.len().max(1) as f32
    }
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Cu8 { .. } => "cu8",
            Self::Cf32 => "cf32",
            Self::Audio(Audio {
                container: Container::Wav,
                ..
            }) => "wav",
            Self::Audio(Audio {
                container: Container::Raw,
                ..
            }) => "s16",
        }
    }
}

pub fn write(
    fixture: &Fixture,
    offset_hz: f64,
    format: Format,
    loops: usize,
    tail_s: f64,
    path: &Path,
) -> Result<()> {
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut sink = BufWriter::new(file);
    match format {
        Format::Cu8 { rate } => write_cu8(fixture, rate, loops, tail_s, &mut sink)?,
        Format::Cf32 => fixture.feed(loops, tail_s, |block| {
            for s in block {
                sink.write_all(&s.re.to_le_bytes())?;
                sink.write_all(&s.im.to_le_bytes())?;
            }
            Ok(())
        })?,
        Format::Audio(audio) => write_audio(fixture, offset_hz, audio, loops, tail_s, &mut sink)?,
    }
    sink.flush()?;
    Ok(())
}

fn write_cu8(
    fixture: &Fixture,
    rate: f64,
    loops: usize,
    tail_s: f64,
    sink: &mut impl Write,
) -> Result<()> {
    ensure!(
        (rate - fixture.rate).abs() < 1e-6,
        "cu8 is written at the fixture rate, {} Hz, not {rate} Hz",
        fixture.rate
    );
    fixture.feed(loops, tail_s, |block| {
        for s in block {
            sink.write_all(&[unsigned(s.re), unsigned(s.im)])?;
        }
        Ok(())
    })
}

fn resample(iq: &[Complex<f32>], length: usize) -> Vec<Complex<f32>> {
    let mut planner = FftPlanner::<f32>::new();
    let mut spectrum = iq.to_vec();
    planner
        .plan_fft_forward(spectrum.len())
        .process(&mut spectrum);
    let kept = iq.len().min(length) / 2;
    let mut resized = vec![Complex::default(); length];
    resized[..kept].copy_from_slice(&spectrum[..kept]);
    resized[length - kept..].copy_from_slice(&spectrum[iq.len() - kept..]);
    planner.plan_fft_inverse(length).process(&mut resized);
    let scale = 1.0 / iq.len() as f32;
    resized.iter_mut().for_each(|s| *s *= scale);
    resized
}

fn dequantised(value: u8) -> f32 {
    (f32::from(value) - 127.5) / 127.5
}

fn unsigned(value: f32) -> u8 {
    (value * 127.5 + 127.5).round().clamp(0.0, 255.0) as u8
}

fn pcm(value: f32) -> i16 {
    (value * f32::from(i16::MAX))
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

struct AudioChain {
    ddc: Option<Ddc>,
    demod: Demod,
    fm: Option<FmDemod>,
    carrier: f32,
    resampler: Option<FracResampler>,
    tuned: Vec<Complex<f32>>,
    audio: Vec<f32>,
    lifted: Vec<Complex<f32>>,
    resampled: Vec<Complex<f32>>,
}

impl AudioChain {
    fn new(fixture: &Fixture, offset_hz: f64, demod: Demod, rate: u32) -> Result<Self> {
        let source_rate = match demod {
            Demod::Real => fixture.rate,
            Demod::Fm { .. } | Demod::Am => AUDIO_RATE,
        };
        let ddc = match demod {
            Demod::Real => None,
            Demod::Fm { .. } | Demod::Am => Some(
                Ddc::new(fixture.rate, AUDIO_RATE, offset_hz).map_err(|err| anyhow!("{err}"))?,
            ),
        };
        let fm = match demod {
            Demod::Fm { deviation_hz } => Some(FmDemod::new(AUDIO_RATE, deviation_hz)),
            Demod::Am | Demod::Real => None,
        };
        let ratio = f64::from(rate) / source_rate;
        Ok(Self {
            ddc,
            demod,
            fm,
            carrier: fixture.mean_magnitude().max(f32::MIN_POSITIVE),
            resampler: ((ratio - 1.0).abs() > 1e-9).then(|| FracResampler::new(ratio)),
            tuned: Vec::new(),
            audio: Vec::new(),
            lifted: Vec::new(),
            resampled: Vec::new(),
        })
    }

    fn process(&mut self, block: &[Complex<f32>]) -> &[f32] {
        let tuned = match &mut self.ddc {
            Some(ddc) => {
                ddc.process(block, &mut self.tuned);
                self.tuned.as_slice()
            }
            None => block,
        };
        self.audio.clear();
        match (self.demod, &mut self.fm) {
            (Demod::Fm { .. }, Some(fm)) => {
                fm.process(tuned, &mut self.audio);
                self.audio.iter_mut().for_each(|a| *a *= AUDIO_GAIN);
            }
            (Demod::Am, _) => self.audio.extend(
                tuned
                    .iter()
                    .map(|s| (s.norm() - self.carrier) / self.carrier * AUDIO_GAIN),
            ),
            _ => self.audio.extend(tuned.iter().map(|s| s.re)),
        }
        let Some(resampler) = &mut self.resampler else {
            return &self.audio;
        };
        self.lifted.clear();
        self.lifted
            .extend(self.audio.iter().map(|&a| Complex::new(a, 0.0)));
        resampler.process(&self.lifted, &mut self.resampled);
        self.audio.clear();
        self.audio.extend(self.resampled.iter().map(|s| s.re));
        &self.audio
    }
}

fn write_audio<W: Write + Seek>(
    fixture: &Fixture,
    offset_hz: f64,
    audio: Audio,
    loops: usize,
    tail_s: f64,
    sink: &mut W,
) -> Result<()> {
    let mut chain = AudioChain::new(fixture, offset_hz, audio.demod, audio.rate)?;
    if let Container::Wav = audio.container {
        sink.write_all(&wav_header(audio.rate, 0))?;
    }
    let mut samples = 0u32;
    fixture.feed(loops, tail_s, |block| {
        for &a in chain.process(block) {
            sink.write_all(&pcm(a).to_le_bytes())?;
            samples += 1;
        }
        Ok(())
    })?;
    if let Container::Wav = audio.container {
        sink.seek(SeekFrom::Start(0))?;
        sink.write_all(&wav_header(audio.rate, samples))?;
    }
    Ok(())
}

fn wav_header(rate: u32, samples: u32) -> Vec<u8> {
    let data = samples * 2;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&rate.to_le_bytes());
    header.extend_from_slice(&(rate * 2).to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data.to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(iq: Vec<Complex<f32>>, rate: f64) -> Fixture {
        Fixture {
            iq,
            rate,
            zeros: vec![Complex::default(); BLOCK],
        }
    }

    #[test]
    fn a_wav_header_describes_mono_16_bit() {
        let header = wav_header(12_500, 10);
        assert_eq!(header.len(), 44);
        assert_eq!(&header[40..44], &20u32.to_le_bytes());
        assert_eq!(&header[24..28], &12_500u32.to_le_bytes());
    }

    #[test]
    fn samples_quantise_to_full_scale() {
        assert_eq!(unsigned(1.0), 255);
        assert_eq!(unsigned(-1.0), 0);
        assert_eq!(unsigned(0.0), 128);
        assert_eq!(pcm(2.0), i16::MAX);
        assert_eq!(pcm(-2.0), i16::MIN);
    }

    #[test]
    fn feeding_loops_the_signal_then_adds_silence() {
        let signal = fixture(vec![Complex::new(1.0, 0.0); 3], 10.0);
        let mut fed = Vec::new();
        signal
            .feed(2, 0.5, |block| {
                fed.extend_from_slice(block);
                Ok(())
            })
            .expect("feed");
        assert_eq!(fed.len(), 11);
        assert_eq!(fed[6], Complex::default());
    }

    #[test]
    fn cu8_round_trips_exactly() {
        for value in [0u8, 1, 127, 128, 254, 255] {
            assert_eq!(unsigned(dequantised(value)), value);
        }
    }

    #[test]
    fn resampling_keeps_a_tone() {
        let n = 1_000;
        let tone: Vec<_> = (0..n)
            .map(|k| Complex::from_polar(0.5, std::f32::consts::TAU * 50.0 * k as f32 / n as f32))
            .collect();
        let up = fixture(tone, 2_000_000.0)
            .as_cu8(2_400_000.0)
            .expect("resample");
        assert_eq!(up.iq.len(), 1_200);
        assert!((up.rate - 2_400_000.0).abs() < 1e-9);
        let expected = Complex::from_polar(0.5, std::f32::consts::TAU * 50.0 * 600.0 / 1_200.0);
        assert!((up.iq[600] - expected).norm() < 0.02, "{}", up.iq[600]);
    }

    #[test]
    fn a_real_signal_is_written_as_its_samples() {
        let dir = std::env::temp_dir().join(format!("sdrmm-convert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("real.wav");
        let signal = fixture(
            vec![Complex::new(0.5, 0.0), Complex::new(-0.5, 0.0)],
            12_000.0,
        );
        let format = Format::Audio(Audio {
            demod: Demod::Real,
            rate: 12_000,
            container: Container::Wav,
        });
        write(&signal, 0.0, format, 1, 0.0, &path).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(bytes.len(), 48);
        assert_eq!(&bytes[44..46], &16_384i16.to_le_bytes());
        std::fs::remove_dir_all(&dir).expect("clean");
    }
}
