use super::MobileCore;
use crate::records::{HeadingSample, LocationSample, MotionSample, PoseSettings};

const FEATURE: &str = "Pose";

#[uniffi::export]
impl MobileCore {
    pub fn push_location(&self, sample: LocationSample) {
        self.inner.not_built(FEATURE, sample);
    }

    pub fn push_heading(&self, sample: HeadingSample) {
        self.inner.not_built(FEATURE, sample);
    }

    pub fn push_motion(&self, sample: MotionSample) {
        self.inner.not_built(FEATURE, sample);
    }

    pub fn set_pose_settings(&self, settings: PoseSettings) {
        self.inner.not_built(FEATURE, settings);
    }

    pub fn start_align(&self) {
        self.inner.not_built(FEATURE, ());
    }

    pub fn cancel_align(&self) {
        self.inner.not_built(FEATURE, ());
    }
}
