//! Per-channel picture hand-off (PLAN §13: ATV). The counterpart of [`crate::audio`], and
//! deliberately thinner: a picture is 8-bit luma a client draws straight into a canvas, so there
//! is no encoder thread between the DSP plane and the socket — what the demodulator scanned out
//! is what the WebSocket sends.

use std::sync::Arc;

use sdrmm_channels::VideoPicture;

/// Fields arrive at 50–60 Hz and a picture is tens of kilobytes, so eight buffers is a sixth of
/// a second of slack before the drop-oldest contract sheds the stale ones (PLAN §5). Deeper
/// would only mean handing a client older pictures: nothing downstream wants anything but the
/// newest one.
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
