use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const AUDIO_SUFFIX: &str = ".wav";
pub const AUDIO_BYTES_PER_SAMPLE: u64 = 2;
const HEADER_LEN: u64 = 44;
const FORMAT_PCM: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const HEADER_REFRESH_FRAMES: u64 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioInfo {
    pub channels: u8,
    pub sample_rate: u32,
    pub frames: u64,
    pub bytes: u64,
}

impl AudioInfo {
    #[must_use]
    pub fn duration_s(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / f64::from(self.sample_rate)
        }
    }
}

#[derive(Debug)]
pub struct AudioWriter {
    file: File,
    path: PathBuf,
    channels: u8,
    frames: u64,
    refreshed_at: u64,
    scratch: Vec<u8>,
}

impl AudioWriter {
    pub fn create(path: &Path, sample_rate: u32, channels: u8) -> io::Result<Self> {
        if channels == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an audio recording needs at least one channel",
            ));
        }
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&header(sample_rate, channels, 0))?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            channels,
            frames: 0,
            refreshed_at: 0,
            scratch: Vec::new(),
        })
    }

    pub fn write_frames(&mut self, interleaved: &[f32]) -> io::Result<()> {
        let channels = usize::from(self.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} samples is not whole frames of {channels}-channel audio",
                    interleaved.len()
                ),
            ));
        }
        self.scratch.clear();
        self.scratch.reserve(interleaved.len() * 2);
        for &sample in interleaved {
            self.scratch
                .extend_from_slice(&to_pcm16(sample).to_le_bytes());
        }
        self.file.write_all(&self.scratch)?;
        self.frames += (interleaved.len() / channels) as u64;
        self.refresh_header()
    }

    pub fn write_silence(&mut self, frames: usize) -> io::Result<()> {
        if frames == 0 {
            return Ok(());
        }
        self.scratch.clear();
        self.scratch
            .resize(frames * usize::from(self.channels) * 2, 0);
        self.file.write_all(&self.scratch)?;
        self.frames += frames as u64;
        self.refresh_header()
    }

    #[must_use]
    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.frames * u64::from(self.channels) * AUDIO_BYTES_PER_SAMPLE
    }

    #[must_use]
    pub fn channels(&self) -> u8 {
        self.channels
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn finalize(mut self) -> io::Result<()> {
        self.write_sizes()?;
        self.file.sync_all()
    }

    fn refresh_header(&mut self) -> io::Result<()> {
        if self.frames - self.refreshed_at < HEADER_REFRESH_FRAMES {
            return Ok(());
        }
        self.write_sizes()
    }

    fn write_sizes(&mut self) -> io::Result<()> {
        let data_len = self.bytes_written();
        let riff = u32::try_from(HEADER_LEN - 8 + data_len).unwrap_or(u32::MAX);
        let data = u32::try_from(data_len).unwrap_or(u32::MAX);
        self.file.seek(SeekFrom::Start(4))?;
        self.file.write_all(&riff.to_le_bytes())?;
        self.file.seek(SeekFrom::Start(HEADER_LEN - 4))?;
        self.file.write_all(&data.to_le_bytes())?;
        self.file.seek(SeekFrom::End(0))?;
        self.refreshed_at = self.frames;
        Ok(())
    }
}

pub fn scan_audio(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut files = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        if name.to_str().is_some_and(|n| n.ends_with(AUDIO_SUFFIX)) {
            files.push(dir.join(name));
        }
    }
    files.sort();
    Ok(files)
}

pub fn read_audio_info(path: &Path) -> io::Result<AudioInfo> {
    let mut file = File::open(path)?;
    let mut head = [0u8; HEADER_LEN as usize];
    file.read_exact(&mut head)?;
    let invalid = |what: &str| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {what}", path.display()),
        )
    };
    if &head[..4] != b"RIFF" || &head[8..12] != b"WAVE" || &head[12..16] != b"fmt " {
        return Err(invalid("not a canonical PCM WAV"));
    }
    let format = u16::from_le_bytes([head[20], head[21]]);
    let bits = u16::from_le_bytes([head[34], head[35]]);
    if format != FORMAT_PCM || bits != BITS_PER_SAMPLE {
        return Err(invalid("only 16-bit PCM audio is read here"));
    }
    let channels = u16::from_le_bytes([head[22], head[23]]);
    let sample_rate = u32::from_le_bytes([head[24], head[25], head[26], head[27]]);
    let channels = u8::try_from(channels).ok().filter(|&c| c > 0);
    let Some(channels) = channels else {
        return Err(invalid("no channel count"));
    };
    let bytes = file.metadata()?.len().saturating_sub(HEADER_LEN);
    let frame_bytes = u64::from(channels) * AUDIO_BYTES_PER_SAMPLE;
    Ok(AudioInfo {
        channels,
        sample_rate,
        frames: bytes / frame_bytes,
        bytes: bytes - bytes % frame_bytes,
    })
}

fn header(sample_rate: u32, channels: u8, data_len: u32) -> Vec<u8> {
    let block_align = u16::from(channels) * BITS_PER_SAMPLE / 8;
    let mut out = Vec::with_capacity(HEADER_LEN as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(HEADER_LEN as u32 - 8 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
    out.extend_from_slice(&u16::from(channels).to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(
        &sample_rate
            .saturating_mul(u32::from(block_align))
            .to_le_bytes(),
    );
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out
}

fn to_pcm16(sample: f32) -> i16 {
    if !sample.is_finite() {
        return 0;
    }
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const RATE: u32 = 48_000;

    fn samples(path: &Path) -> Vec<i16> {
        let bytes = fs::read(path).expect("read wav");
        bytes[HEADER_LEN as usize..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect()
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
    }

    #[test]
    fn a_finalized_file_is_mono_48k_pcm_with_the_samples_that_were_written() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("voice.wav");
        let mut writer = AudioWriter::create(&path, RATE, 1).expect("create");
        writer.write_frames(&[0.0, 1.0, -1.0, 0.5]).expect("write");
        writer.write_silence(2).expect("silence");
        assert_eq!(writer.frames_written(), 6);
        assert_eq!(writer.bytes_written(), 12);
        writer.finalize().expect("finalize");

        let bytes = fs::read(&path).expect("read");
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8);
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u32_at(&bytes, 24), RATE);
        assert_eq!(
            u32_at(&bytes, 40) as usize,
            bytes.len() - HEADER_LEN as usize
        );
        assert_eq!(
            samples(&path),
            [0, i16::MAX, -i16::MAX, 16_384, 0, 0],
            "samples were not written at full scale"
        );

        let info = read_audio_info(&path).expect("info");
        assert_eq!(
            info,
            AudioInfo {
                channels: 1,
                sample_rate: RATE,
                frames: 6,
                bytes: 12,
            }
        );
        assert!((info.duration_s() - 6.0 / f64::from(RATE)).abs() < 1e-9);
    }

    #[test]
    fn stereo_frames_carry_both_sides() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("stereo.wav");
        let mut writer = AudioWriter::create(&path, RATE, 2).expect("create");
        writer.write_frames(&[1.0, -1.0, 0.5, -0.5]).expect("write");
        writer.finalize().expect("finalize");

        let info = read_audio_info(&path).expect("info");
        assert_eq!((info.channels, info.frames, info.bytes), (2, 2, 8));
        assert_eq!(samples(&path), [i16::MAX, -i16::MAX, 16_384, -16_384]);
    }

    #[test]
    fn a_ragged_block_is_refused_rather_than_written_half_a_frame_short() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("ragged.wav");
        let mut writer = AudioWriter::create(&path, RATE, 2).expect("create");
        let err = writer.write_frames(&[1.0, -1.0, 0.5]).expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(writer.frames_written(), 0);
    }

    #[test]
    fn a_writer_that_never_finalizes_leaves_a_playable_file() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("killed.wav");
        let mut writer = AudioWriter::create(&path, RATE, 1).expect("create");
        writer
            .write_frames(&vec![0.25; 2 * HEADER_REFRESH_FRAMES as usize])
            .expect("write");
        drop(writer);

        let bytes = fs::read(&path).expect("read");
        assert_eq!(
            u32_at(&bytes, 40) as u64,
            2 * HEADER_REFRESH_FRAMES * AUDIO_BYTES_PER_SAMPLE
        );
        let info = read_audio_info(&path).expect("info");
        assert_eq!(info.frames, 2 * HEADER_REFRESH_FRAMES);
    }

    #[test]
    fn out_of_range_and_non_finite_samples_are_bounded() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("hot.wav");
        let mut writer = AudioWriter::create(&path, RATE, 1).expect("create");
        writer
            .write_frames(&[4.0, -4.0, f32::NAN, f32::INFINITY])
            .expect("write");
        writer.finalize().expect("finalize");
        assert_eq!(samples(&path), [i16::MAX, -i16::MAX, 0, 0]);
    }

    #[test]
    fn a_claimed_name_is_refused_rather_than_shared() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("taken.wav");
        let writer = AudioWriter::create(&path, RATE, 1).expect("create");
        writer.finalize().expect("finalize");
        let err = AudioWriter::create(&path, RATE, 1).expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn the_library_lists_only_finished_looking_wavs_and_survives_a_missing_directory() {
        let dir = TempDir::new().expect("tempdir");
        assert!(
            scan_audio(&dir.path().join("never-recorded"))
                .expect("missing dir")
                .is_empty()
        );
        for name in ["b.wav", "a.wav"] {
            AudioWriter::create(&dir.path().join(name), RATE, 1)
                .expect("create")
                .finalize()
                .expect("finalize");
        }
        fs::write(dir.path().join("notes.txt"), b"not audio").expect("write");
        let listed: Vec<String> = scan_audio(dir.path())
            .expect("scan")
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .collect();
        assert_eq!(listed, ["a.wav", "b.wav"]);
    }

    #[test]
    fn a_file_that_is_not_a_pcm_wav_is_named_rather_than_guessed_at() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("bogus.wav");
        fs::write(&path, vec![0u8; HEADER_LEN as usize]).expect("write");
        let err = read_audio_info(&path).expect_err("refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
