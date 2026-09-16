use std::sync::{Mutex, MutexGuard};

use sdrmm_device::{DeviceError, lock};
use sdrmm_wire::DeviceSettings;

use super::CaptureRuntime;

/// A radio and the settings still waiting to reach it.
///
/// A dial spun through fifty positions asks for fifty tunings, and a tuner reprogrammed once per
/// position answers none of them: every one but the last is already stale by the time the radio is
/// free. Patches queue together instead, and whichever one reaches the radio carries what all of
/// them asked for, so nothing is programmed for a setting that has already been superseded.
pub(crate) struct DeviceRuntime {
    runtime: Mutex<CaptureRuntime>,
    waiting: Mutex<Waiting>,
    patching: Mutex<()>,
    sweeping: bool,
}

impl DeviceRuntime {
    pub(crate) fn new(runtime: CaptureRuntime) -> Self {
        Self {
            sweeping: runtime.is_sweeping(),
            runtime: Mutex::new(runtime),
            waiting: Mutex::new(Waiting::default()),
            patching: Mutex::new(()),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, CaptureRuntime> {
        lock(&self.runtime)
    }

    pub(crate) const fn sweeping(&self) -> bool {
        self.sweeping
    }

    /// Held from reading what the radio is set to until the answer is written back, so a patch
    /// that takes milliseconds at the tuner cannot be overtaken by one built from the settings it
    /// is in the middle of replacing.
    pub(crate) fn patching(&self) -> MutexGuard<'_, ()> {
        lock(&self.patching)
    }

    /// Applies `hardware`, or nothing at all when a patch carrying it already reached the radio
    /// while this one waited its turn. Answers with the settings the radio is left holding, and
    /// with the refusal of the patch that carried these settings when that is what happened.
    pub(crate) fn apply(
        &self,
        hardware: &DeviceSettings,
    ) -> Result<Option<DeviceSettings>, DeviceError> {
        let batch = lock(&self.waiting).join(hardware.clone());
        let mut runtime = self.lock();
        let owed = lock(&self.waiting).take();
        let Some(settings) = owed else {
            let refusal = lock(&self.waiting).refusal(batch);
            return match refusal {
                Some(error) => Err(error),
                None => Ok(runtime.device_settings()),
            };
        };
        if let Err(error) = runtime.apply(&settings) {
            lock(&self.waiting).refuse(batch, error.clone());
            return Err(error);
        }
        Ok(runtime.device_settings())
    }
}

/// The settings owed to a radio, and what became of the last lot that was not applied.
#[derive(Default)]
struct Waiting {
    settings: Option<DeviceSettings>,
    batch: u64,
    refused: Option<(u64, DeviceError)>,
}

impl Waiting {
    /// Adds settings to what the radio is owed, answering with the batch they joined.
    fn join(&mut self, settings: DeviceSettings) -> u64 {
        match self.settings.as_mut() {
            Some(owed) => owed.merge_from(&settings),
            None => self.settings = Some(settings),
        }
        self.batch
    }

    fn take(&mut self) -> Option<DeviceSettings> {
        let settings = self.settings.take()?;
        self.batch += 1;
        Some(settings)
    }

    fn refuse(&mut self, batch: u64, error: DeviceError) {
        self.refused = Some((batch, error));
    }

    fn refusal(&self, batch: u64) -> Option<DeviceError> {
        self.refused
            .as_ref()
            .filter(|(refused, _)| *refused == batch)
            .map(|(_, error)| error.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuned(hz: f64) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(hz),
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn what_reaches_the_radio_is_what_the_last_patch_asked_for() {
        let mut waiting = Waiting::default();
        waiting.join(tuned(100e6));
        waiting.join(tuned(101e6));
        assert_eq!(
            waiting.take().expect("settings are owed").center_hz,
            Some(101e6)
        );
    }

    #[test]
    fn a_burst_loses_no_setting_only_one_of_its_patches_touched() {
        let mut waiting = Waiting::default();
        waiting.join(DeviceSettings {
            ppm: Some(5.0),
            ..tuned(100e6)
        });
        waiting.join(tuned(101e6));
        let owed = waiting.take().expect("settings are owed");
        assert_eq!(owed.center_hz, Some(101e6));
        assert_eq!(owed.ppm, Some(5.0), "nothing asked for undid this");
    }

    #[test]
    fn a_patch_whose_settings_already_landed_applies_nothing() {
        let mut waiting = Waiting::default();
        waiting.join(tuned(100e6));
        assert!(waiting.take().is_some());
        assert!(waiting.take().is_none());
    }

    #[test]
    fn a_refusal_answers_every_patch_that_was_carried_by_it() {
        let mut waiting = Waiting::default();
        let batch = waiting.join(tuned(100e6));
        let alongside = waiting.join(tuned(101e6));
        waiting.take().expect("settings are owed");
        waiting.refuse(batch, DeviceError::Unsupported("no".to_string()));
        assert!(waiting.refusal(batch).is_some());
        assert!(waiting.refusal(alongside).is_some());
    }

    #[test]
    fn a_refused_patch_is_not_retried_under_the_one_after_it() {
        let mut waiting = Waiting::default();
        let batch = waiting.join(tuned(100e6));
        waiting.take().expect("settings are owed");
        waiting.refuse(batch, DeviceError::Unsupported("no".to_string()));

        let next = waiting.join(tuned(101e6));
        assert_ne!(next, batch, "a later patch is a batch of its own");
        assert!(
            waiting.refusal(next).is_none(),
            "settings the radio refused must not follow the patches after them"
        );
        assert_eq!(
            waiting.take().expect("settings are owed").center_hz,
            Some(101e6)
        );
    }
}
