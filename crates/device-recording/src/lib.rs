use std::path::{Path, PathBuf};

use sdrmm_device::{DeviceDriver, DeviceError, SdrDevice};
use sdrmm_recorder::scan_stems;
use sdrmm_wire::{DeviceInfo, RECORDING_DRIVER_ID, recording_stem_valid};

mod playback;
pub use playback::{FilePlayback, LOOP_SETTING};

pub(crate) const DRIVER_ID: &str = RECORDING_DRIVER_ID;
pub(crate) const BLOCK_SECS: f64 = 0.025;

pub struct RecordingDriver {
    dir: Option<PathBuf>,
    playback_speed: f64,
}

fn checked_speed(playback_speed: f64) -> f64 {
    assert!(
        playback_speed.is_finite() && playback_speed >= 1.0,
        "playback speed must be finite and at least real time"
    );
    playback_speed
}

impl Default for RecordingDriver {
    fn default() -> Self {
        Self::new(None)
    }
}

impl RecordingDriver {
    #[must_use]
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            playback_speed: 1.0,
        }
    }

    #[must_use]
    pub fn accelerated(dir: Option<PathBuf>, playback_speed: f64) -> Self {
        Self {
            dir,
            playback_speed: checked_speed(playback_speed),
        }
    }

    fn stem_path(&self, stem: &str) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        recording_stem_valid(stem).then(|| dir.join(stem))
    }

    fn info(stem: &str) -> DeviceInfo {
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: stem.to_string(),
            label: stem.to_string(),
            serial: None,
            profile: None,
        }
    }
}

impl DeviceDriver for RecordingDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let Ok(stems) = scan_stems(dir) else {
            return Vec::new();
        };
        stems
            .iter()
            .filter_map(|stem| stem.file_name()?.to_str().map(Self::info))
            .collect()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let path = self
            .stem_path(&info.key)
            .ok_or_else(|| DeviceError::NotFound(format!("{DRIVER_ID}:{}", info.key)))?;
        Ok(Box::new(FilePlayback::open_at_speed(
            &path,
            self.playback_speed,
        )?))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        let path = self.stem_path(key)?;
        sdrmm_recorder::meta_path(&path).exists().then(|| {
            let mut info = Self::info(key);
            info.label = path.file_name()?.to_str()?.to_string();
            Some(info)
        })?
    }
}

pub fn open_at(stem: &Path) -> Result<FilePlayback, DeviceError> {
    FilePlayback::open(stem)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sdrmm_recorder::SigmfWriter;
    use tempfile::TempDir;

    use super::*;

    fn record(dir: &Path, name: &str) -> PathBuf {
        let stem = dir.join(name);
        let mut writer =
            SigmfWriter::create(&stem, 250_000.0, 100_000_000.0, "test").expect("create");
        writer
            .write_block(&[num_complex::Complex::new(0.5f32, -0.5)])
            .expect("write");
        writer.finalize().expect("finalize");
        stem
    }

    #[test]
    fn probe_names_every_recording_by_its_stem_alone() {
        let dir = TempDir::new().expect("temp");
        record(dir.path(), "first");
        record(dir.path(), "second");
        let driver = RecordingDriver::new(Some(dir.path().to_path_buf()));

        let ids: Vec<String> = driver.probe().iter().map(DeviceInfo::id).collect();
        assert_eq!(ids, vec!["recording:first", "recording:second"]);
    }

    #[test]
    fn a_library_that_is_not_there_holds_no_recordings() {
        assert!(RecordingDriver::new(None).probe().is_empty());
        let missing = TempDir::new().expect("temp").path().join("gone");
        assert!(RecordingDriver::new(Some(missing)).probe().is_empty());
    }

    #[test]
    fn a_stem_that_reaches_out_of_the_library_is_refused() {
        let dir = TempDir::new().expect("temp");
        record(dir.path(), "inside");
        let outside = dir.path().parent().expect("parent").join("outside");
        fs::write(sdrmm_recorder::meta_path(&outside), "{}").expect("write");
        let driver = RecordingDriver::new(Some(dir.path().to_path_buf()));

        for key in ["../outside", "/etc/passwd", "sub/dir", "..", ""] {
            assert!(driver.resolve(key).is_none(), "{key} must be refused");
            assert!(
                driver.open(&RecordingDriver::info(key)).is_err(),
                "{key} must be refused"
            );
        }
        let _ = fs::remove_file(sdrmm_recorder::meta_path(&outside));
    }

    #[test]
    fn resolve_names_a_recording_the_last_probe_missed() {
        let dir = TempDir::new().expect("temp");
        let driver = RecordingDriver::new(Some(dir.path().to_path_buf()));
        assert!(driver.resolve("late").is_none());

        record(dir.path(), "late");
        assert_eq!(
            driver.resolve("late").map(|info| info.id()),
            Some("recording:late".to_string())
        );
    }

    #[test]
    fn open_plays_the_recording_the_stem_names() {
        let dir = TempDir::new().expect("temp");
        record(dir.path(), "played");
        let driver = RecordingDriver::new(Some(dir.path().to_path_buf()));

        let device = driver
            .open(&RecordingDriver::info("played"))
            .expect("opens");
        assert_eq!(device.settings().sample_rate, Some(250_000.0));
    }
}
