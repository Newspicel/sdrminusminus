use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use num_complex::Complex;
use sdrmm_dsp::Ddc;
use sha2::{Digest, Sha256};

const BLOCK: usize = 65_536;

#[derive(Args)]
pub struct Excerpt {
    pub input: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, default_value_t = 0.0)]
    pub start: f64,
    #[arg(long)]
    pub duration: f64,
    #[arg(long, default_value_t = 0.0)]
    pub offset: f64,
    #[arg(long)]
    pub rate: Option<f64>,
    #[arg(long)]
    pub input_rate: Option<f64>,
    #[arg(long)]
    pub center: Option<f64>,
    #[arg(long)]
    pub description: String,
    #[arg(long)]
    pub hw: String,
    #[arg(long)]
    pub source: String,
}

pub fn run(root: &Path, args: &Excerpt) -> Result<()> {
    let mut source = Source::open(&args.input, args.input_rate, args.center)?;
    let input_rate = source.rate;
    let output_rate = args.rate.unwrap_or(input_rate);
    ensure!(
        output_rate <= input_rate,
        "output rate {output_rate} Hz is above the recording's {input_rate} Hz"
    );

    let skip = (args.start * input_rate).round() as u64;
    let take = (args.duration * input_rate).round() as u64;
    ensure!(take > 0, "--duration must cover at least one sample");
    source.seek(skip)?;

    let mut ddc = (output_rate != input_rate || args.offset != 0.0)
        .then(|| Ddc::new(input_rate, output_rate, args.offset))
        .transpose()
        .context("open the down-converter")?;

    let stem = root.join("fixtures").join(&args.out);
    for path in [
        sdrmm_recorder::meta_path(&stem),
        sdrmm_recorder::data_path(&stem),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("replace {}", path.display())),
        }
    }
    let center = source.center + args.offset;
    let mut writer = sdrmm_recorder::SigmfWriter::create(&stem, output_rate, center, &args.hw)
        .with_context(|| format!("create fixture {}", args.out.display()))?;

    let mut read = 0u64;
    let mut written = 0u64;
    let mut block = vec![Complex::default(); BLOCK];
    let mut resampled = Vec::new();
    while read < take {
        let want = BLOCK.min((take - read) as usize);
        let got = source.read(&mut block[..want])?;
        if got == 0 {
            break;
        }
        read += got as u64;
        let out = match ddc.as_mut() {
            Some(ddc) => {
                ddc.process(&block[..got], &mut resampled);
                resampled.as_slice()
            }
            None => &block[..got],
        };
        writer.write_block(out).context("write the excerpt")?;
        written += out.len() as u64;
    }
    writer.finalize().context("finalize the excerpt")?;
    ensure!(
        read == take,
        "the recording ran out after {:.3} s of the {:.3} s asked for",
        read as f64 / input_rate,
        args.duration
    );

    let digest = digest_of(&source.data_path)?;
    let note = format!(
        "{}; source SHA-256 {digest}; samples {}..{} at {} Hz",
        args.source,
        skip,
        skip + take,
        input_rate
    );
    stamp(&stem, &args.description, &note, written)?;

    println!(
        "{}: {written} samples, {:.3} s @ {} — {}",
        args.out.display(),
        written as f64 / output_rate,
        rate_label(output_rate),
        args.description
    );
    Ok(())
}

fn rate_label(rate: f64) -> String {
    if rate >= 1e6 {
        format!("{:.3} Msps", rate / 1e6)
    } else {
        format!("{:.0} ksps", rate / 1e3)
    }
}

fn stamp(stem: &Path, description: &str, note: &str, samples: u64) -> Result<()> {
    let reader = sdrmm_recorder::SigmfReader::open(stem).context("re-open the excerpt")?;
    ensure!(
        reader.total_samples() == samples,
        "excerpt readback: {} samples on disk, {samples} written",
        reader.total_samples()
    );
    let mut meta = reader.meta().clone();
    drop(reader);
    meta.global.description = Some(description.to_owned());
    meta.annotations = vec![serde_json::json!({
        "core:sample_start": 0,
        "core:sample_count": samples,
        "core:comment": note,
    })];
    std::fs::write(
        sdrmm_recorder::meta_path(stem),
        serde_json::to_string_pretty(&meta)?,
    )
    .context("rewrite the excerpt metadata")?;
    Ok(())
}

fn digest_of(path: &Path) -> Result<String> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let got = file.read(&mut buf)?;
        if got == 0 {
            break;
        }
        hasher.update(&buf[..got]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub struct Source {
    reader: Reader,
    pub rate: f64,
    pub center: f64,
    data_path: PathBuf,
}

enum Reader {
    Sigmf(Box<sdrmm_recorder::SigmfReader>),
    Wav(Wav),
    Raw(Raw),
}

impl Source {
    pub fn open(path: &Path, rate: Option<f64>, center: Option<f64>) -> Result<Self> {
        if let Some(format) = RawFormat::of(path) {
            let rate = rate.context("a raw IQ file carries no sample rate — pass --input-rate")?;
            return Ok(Self {
                reader: Reader::Raw(Raw::open(path, format)?),
                rate,
                center: center.unwrap_or(0.0),
                data_path: path.to_path_buf(),
            });
        }
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        {
            let wav = Wav::open(path)?;
            let rate = rate.unwrap_or(wav.rate);
            return Ok(Self {
                reader: Reader::Wav(wav),
                rate,
                center: center.unwrap_or(0.0),
                data_path: path.to_path_buf(),
            });
        }
        let stem = strip_sigmf_suffix(path);
        let reader = sdrmm_recorder::SigmfReader::open(&stem)
            .with_context(|| format!("open {}", stem.display()))?;
        let meta_rate = reader.meta().global.sample_rate;
        let meta_center = reader
            .meta()
            .captures
            .first()
            .and_then(|capture| capture.frequency);
        let rate = rate
            .or(meta_rate)
            .context("the recording carries no sample rate — pass --input-rate")?;
        Ok(Self {
            reader: Reader::Sigmf(Box::new(reader)),
            rate,
            center: center.or(meta_center).unwrap_or(0.0),
            data_path: sdrmm_recorder::data_path(&stem),
        })
    }

    pub fn seek(&mut self, sample: u64) -> Result<()> {
        match &mut self.reader {
            Reader::Sigmf(reader) => reader.seek_to(sample)?,
            Reader::Wav(wav) => wav.seek(sample)?,
            Reader::Raw(raw) => raw.seek(sample)?,
        }
        Ok(())
    }

    pub fn read(&mut self, buf: &mut [Complex<f32>]) -> Result<usize> {
        Ok(match &mut self.reader {
            Reader::Sigmf(reader) => reader.read_block(buf)?,
            Reader::Wav(wav) => wav.read(buf)?,
            Reader::Raw(raw) => raw.read(buf)?,
        })
    }
}

fn strip_sigmf_suffix(path: &Path) -> PathBuf {
    let name = path.to_string_lossy();
    for suffix in [".sigmf-data", ".sigmf-meta"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return PathBuf::from(stem);
        }
    }
    path.to_path_buf()
}

struct Wav {
    file: BufReader<File>,
    rate: f64,
    channels: u16,
    format: SampleFormat,
    bytes_per_sample: usize,
    data_start: u64,
    frames: u64,
    read_frames: u64,
    scratch: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleFormat {
    Int,
    Float,
}

impl Wav {
    fn open(path: &Path) -> Result<Self> {
        let mut file = BufReader::new(File::open(path)?);
        let mut header = [0u8; 12];
        file.read_exact(&mut header)?;
        ensure!(
            &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE",
            "{} is not a RIFF/WAVE file",
            path.display()
        );
        let mut fmt = None;
        let mut pos = 12u64;
        loop {
            let mut chunk = [0u8; 8];
            if file.read_exact(&mut chunk).is_err() {
                bail!("{} carries no data chunk", path.display());
            }
            let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
            let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
            pos += 8;
            if &id == b"fmt " {
                let mut body = vec![0u8; size as usize];
                file.read_exact(&mut body)?;
                fmt = Some(Fmt::parse(&body)?);
            } else if &id == b"data" {
                let fmt = fmt.context("the data chunk arrives before the format chunk")?;
                let bytes_per_sample = usize::from(fmt.bits_per_sample / 8);
                let frame = bytes_per_sample * usize::from(fmt.channels);
                ensure!(frame > 0, "{} declares zero-width frames", path.display());
                return Ok(Self {
                    file,
                    rate: f64::from(fmt.rate),
                    channels: fmt.channels,
                    format: fmt.format,
                    bytes_per_sample,
                    data_start: pos,
                    frames: size / frame as u64,
                    read_frames: 0,
                    scratch: Vec::new(),
                });
            } else {
                std::io::copy(&mut file.by_ref().take(size), &mut std::io::sink())?;
            }
            pos += size + size % 2;
            if size % 2 == 1 {
                std::io::copy(&mut file.by_ref().take(1), &mut std::io::sink())?;
            }
        }
    }

    fn seek(&mut self, frame: u64) -> Result<()> {
        let frame = frame.min(self.frames);
        let stride = self.bytes_per_sample as u64 * u64::from(self.channels);
        std::io::Seek::seek(
            &mut self.file,
            std::io::SeekFrom::Start(self.data_start + frame * stride),
        )?;
        self.read_frames = frame;
        Ok(())
    }

    fn read(&mut self, buf: &mut [Complex<f32>]) -> Result<usize> {
        let want = buf.len().min((self.frames - self.read_frames) as usize);
        if want == 0 {
            return Ok(0);
        }
        let stride = self.bytes_per_sample * usize::from(self.channels);
        self.scratch.resize(want * stride, 0);
        self.file.read_exact(&mut self.scratch)?;
        for (sample, frame) in buf[..want]
            .iter_mut()
            .zip(self.scratch.chunks_exact(stride))
        {
            let re = self.decode(&frame[..self.bytes_per_sample]);
            let im = if self.channels >= 2 {
                self.decode(&frame[self.bytes_per_sample..2 * self.bytes_per_sample])
            } else {
                0.0
            };
            *sample = Complex::new(re, im);
        }
        self.read_frames += want as u64;
        Ok(want)
    }

    fn decode(&self, bytes: &[u8]) -> f32 {
        match (self.format, bytes.len()) {
            (SampleFormat::Float, 4) => {
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            }
            (SampleFormat::Float, 8) => f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]) as f32,
            (SampleFormat::Int, 1) => (f32::from(bytes[0]) - 128.0) / 128.0,
            (SampleFormat::Int, 2) => {
                f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0
            }
            (SampleFormat::Int, 3) => {
                i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) as f32 / 2_147_483_648.0
            }
            (SampleFormat::Int, 4) => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
            _ => 0.0,
        }
    }
}

struct Fmt {
    format: SampleFormat,
    channels: u16,
    rate: u32,
    bits_per_sample: u16,
}

impl Fmt {
    fn parse(body: &[u8]) -> Result<Self> {
        ensure!(body.len() >= 16, "the format chunk is truncated");
        let mut tag = u16::from_le_bytes([body[0], body[1]]);
        let channels = u16::from_le_bytes([body[2], body[3]]);
        let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
        let bits_per_sample = u16::from_le_bytes([body[14], body[15]]);
        if tag == 0xFFFE {
            ensure!(body.len() >= 26, "the extensible format chunk is truncated");
            tag = u16::from_le_bytes([body[24], body[25]]);
        }
        let format = match tag {
            1 => SampleFormat::Int,
            3 => SampleFormat::Float,
            other => bail!("unsupported WAV format tag {other}"),
        };
        ensure!(
            channels >= 1 && bits_per_sample > 0 && bits_per_sample.is_multiple_of(8),
            "unsupported WAV layout: {channels} channels of {bits_per_sample} bits"
        );
        Ok(Self {
            format,
            channels,
            rate,
            bits_per_sample,
        })
    }
}

#[derive(Clone, Copy)]
enum RawFormat {
    Cu8,
    Cs8,
    Cs16,
    Cf32,
}

impl RawFormat {
    fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "cu8" => Self::Cu8,
            "cs8" | "sc8" => Self::Cs8,
            "cs16" | "sc16" => Self::Cs16,
            "cf32" | "fc32" => Self::Cf32,
            _ => return None,
        })
    }

    fn stride(self) -> usize {
        match self {
            Self::Cu8 | Self::Cs8 => 2,
            Self::Cs16 => 4,
            Self::Cf32 => 8,
        }
    }

    fn sample(self, bytes: &[u8]) -> Complex<f32> {
        match self {
            Self::Cu8 => Complex::new(
                (f32::from(bytes[0]) - 127.5) / 127.5,
                (f32::from(bytes[1]) - 127.5) / 127.5,
            ),
            Self::Cs8 => Complex::new(
                f32::from(bytes[0] as i8) / 128.0,
                f32::from(bytes[1] as i8) / 128.0,
            ),
            Self::Cs16 => Complex::new(
                f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_768.0,
                f32::from(i16::from_le_bytes([bytes[2], bytes[3]])) / 32_768.0,
            ),
            Self::Cf32 => Complex::new(
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            ),
        }
    }
}

struct Raw {
    file: BufReader<File>,
    format: RawFormat,
    samples: u64,
    read: u64,
    scratch: Vec<u8>,
}

impl Raw {
    fn open(path: &Path, format: RawFormat) -> Result<Self> {
        let file = File::open(path)?;
        let samples = file.metadata()?.len() / format.stride() as u64;
        Ok(Self {
            file: BufReader::new(file),
            format,
            samples,
            read: 0,
            scratch: Vec::new(),
        })
    }

    fn seek(&mut self, sample: u64) -> Result<()> {
        let sample = sample.min(self.samples);
        std::io::Seek::seek(
            &mut self.file,
            std::io::SeekFrom::Start(sample * self.format.stride() as u64),
        )?;
        self.read = sample;
        Ok(())
    }

    fn read(&mut self, buf: &mut [Complex<f32>]) -> Result<usize> {
        let want = buf.len().min((self.samples - self.read) as usize);
        if want == 0 {
            return Ok(0);
        }
        let stride = self.format.stride();
        self.scratch.resize(want * stride, 0);
        self.file.read_exact(&mut self.scratch)?;
        for (sample, bytes) in buf[..want]
            .iter_mut()
            .zip(self.scratch.chunks_exact(stride))
        {
            *sample = self.format.sample(bytes);
        }
        self.read += want as u64;
        Ok(want)
    }
}
