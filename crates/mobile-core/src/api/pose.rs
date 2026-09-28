use super::MobileCore;
use crate::{
    pose::PoseInput,
    records::{HeadingSample, LocationSample, MotionSample, PoseSettings},
};

#[uniffi::export]
impl MobileCore {
    pub fn push_location(&self, sample: LocationSample) {
        self.inner.pose.push(PoseInput::Location(sample));
    }

    pub fn push_heading(&self, sample: HeadingSample) {
        self.inner.pose.push(PoseInput::Heading(sample));
    }

    pub fn push_motion(&self, sample: MotionSample) {
        self.inner.pose.push(PoseInput::Motion(sample));
    }

    pub fn set_pose_settings(&self, settings: PoseSettings) {
        self.inner.pose.push(PoseInput::Settings(settings));
    }

    pub fn start_align(&self) {
        self.inner.pose.push(PoseInput::StartAlign);
    }

    pub fn cancel_align(&self) {
        self.inner.pose.push(PoseInput::CancelAlign);
    }
}
