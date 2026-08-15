use std::sync::Arc;

pub use sdrmm_channels::VideoPicture;

pub(crate) const VIDEO_CHANNEL_CAP: usize = 8;

#[derive(Clone, Debug)]
pub struct VideoPacket {
    pub seq: u32,
    pub timestamp: u64,
    pub picture: Arc<VideoPicture>,
}
