use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use sdrmm_wire::SurfaceFrame;
use tokio::sync::broadcast;

pub(crate) const SURFACE_BACKLOG: usize = 4;

pub(crate) type SurfaceItem = (u32, Arc<SurfaceFrame>);

#[derive(Default)]
pub(crate) struct SurfaceHub {
    nodes: Mutex<HashMap<String, broadcast::Sender<SurfaceItem>>>,
}

impl SurfaceHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, broadcast::Sender<SurfaceItem>>> {
        self.nodes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sender(&self, node: &str) -> broadcast::Sender<SurfaceItem> {
        self.lock()
            .entry(node.to_owned())
            .or_insert_with(|| broadcast::channel(SURFACE_BACKLOG).0)
            .clone()
    }

    pub(crate) fn open(&self, node: &str) {
        self.sender(node);
    }

    pub(crate) fn publish(&self, node: &str, seq: u32, frame: Arc<SurfaceFrame>) {
        let _unwatched = self.sender(node).send((seq, frame));
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn subscribe(&self, node: &str) -> Option<broadcast::Receiver<SurfaceItem>> {
        self.lock().get(node).map(broadcast::Sender::subscribe)
    }

    pub(crate) fn forget(&self, node: &str) {
        self.lock().remove(node);
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::FusionGridOwned;
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};

    use super::*;

    fn grid(seq: u32) -> Arc<SurfaceFrame> {
        Arc::new(SurfaceFrame::FusionGrid(FusionGridOwned {
            stream_id: 0,
            seq,
            timestamp: 1,
            south: 52.0,
            west: 13.0,
            north: 52.1,
            east: 13.1,
            cols: 1,
            rows: 1,
            cells: vec![255],
        }))
    }

    #[test]
    fn surface_hub_fans_out_by_node() {
        let hub = SurfaceHub::default();
        assert!(hub.subscribe("tri").is_none());
        hub.open("tri");
        hub.open("radar");
        let mut first = hub.subscribe("tri").expect("an opened node has a surface");
        let mut second = hub.subscribe("tri").expect("a second viewer");
        let mut other = hub.subscribe("radar").expect("radar surface");

        hub.publish("tri", 7, grid(7));
        for receiver in [&mut first, &mut second] {
            let (seq, frame) = receiver.try_recv().expect("each viewer gets the frame");
            assert_eq!(seq, 7);
            assert_eq!(frame, grid(7));
        }
        assert!(matches!(other.try_recv(), Err(TryRecvError::Empty)));

        hub.publish("late", 1, grid(1));
        assert!(hub.subscribe("late").is_some());

        hub.forget("tri");
        assert!(hub.subscribe("tri").is_none());
        assert!(matches!(first.try_recv(), Err(TryRecvError::Closed)));
        assert!(hub.subscribe("radar").is_some());
    }

    #[tokio::test]
    async fn a_slow_viewer_learns_how_many_frames_it_missed() {
        let hub = SurfaceHub::default();
        hub.open("tri");
        let mut slow = hub.subscribe("tri").expect("surface");
        let sent = u32::try_from(SURFACE_BACKLOG).expect("small backlog") + 3;
        for seq in 0..sent {
            hub.publish("tri", seq, grid(seq));
        }
        assert!(matches!(slow.recv().await, Err(RecvError::Lagged(3))));
        let (seq, _) = slow.recv().await.expect("the newest frames remain");
        assert_eq!(seq, 3);
    }
}
