use std::sync::Arc;

use sdrmm_channels::VideoPicture;

pub(crate) const VIDEO_CHANNEL_CAP: usize = 8;

/// One picture on its way to the clients watching a channel.
#[derive(Clone, Debug)]
pub struct VideoPacket {
    /// Pictures since this pipeline started. Restarts when the pipeline is rebuilt (a params
    /// type change, a device rate change) — it counts frames for display, and it is
    /// [`VideoPacket::timestamp`] that stays continuous across the swap.
    pub seq: u32,
    /// Channel-rate sample count when the picture completed, which makes the gap between two
    /// pictures legible as the time it really was.
    pub timestamp: u64,
    /// Shared rather than copied: one picture reaches every subscribed client.
    pub picture: Arc<VideoPicture>,
}
