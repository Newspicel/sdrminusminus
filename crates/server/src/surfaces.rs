use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use sdrmm_wire::{StreamKind, SurfaceFrame};
use tokio::sync::broadcast;

pub(crate) const SURFACE_BACKLOG: usize = 4;

pub(crate) type SurfaceItem = (u32, Arc<SurfaceFrame>);

struct Surface {
    kind: StreamKind,
    frames: broadcast::Sender<SurfaceItem>,
}

impl Surface {
    fn new(kind: StreamKind) -> Self {
        Self {
            kind,
            frames: broadcast::channel(SURFACE_BACKLOG).0,
        }
    }
}

#[derive(Default)]
pub(crate) struct SurfaceHub {
    nodes: Mutex<HashMap<String, Surface>>,
}

impl SurfaceHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Surface>> {
        self.nodes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn open(&self, node: &str, kind: StreamKind) {
        let mut nodes = self.lock();
        match nodes.get_mut(node) {
            Some(surface) if surface.kind == kind => {}
            Some(surface) => *surface = Surface::new(kind),
            None => {
                nodes.insert(node.to_owned(), Surface::new(kind));
            }
        }
    }

    pub(crate) fn publish(&self, node: &str, seq: u32, frame: Arc<SurfaceFrame>) {
        let frames = self
            .lock()
            .entry(node.to_owned())
            .or_insert_with(|| Surface::new(frame.kind()))
            .frames
            .clone();
        let _unwatched = frames.send((seq, frame));
    }

    pub(crate) fn subscribe(
        &self,
        node: &str,
    ) -> Option<(StreamKind, broadcast::Receiver<SurfaceItem>)> {
        self.lock()
            .get(node)
            .map(|surface| (surface.kind, surface.frames.subscribe()))
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
        hub.open("tri", StreamKind::FusionGrid);
        hub.open("radar", StreamKind::RangeDoppler);
        let (_, mut first) = hub.subscribe("tri").expect("an opened node has a surface");
        let (_, mut second) = hub.subscribe("tri").expect("a second viewer");
        let (_, mut other) = hub.subscribe("radar").expect("radar surface");

        hub.publish("tri", 7, grid(7));
        for receiver in [&mut first, &mut second] {
            let (seq, frame) = receiver.try_recv().expect("each viewer gets the frame");
            assert_eq!(seq, 7);
            assert_eq!(frame, grid(7));
        }
        assert!(matches!(other.try_recv(), Err(TryRecvError::Empty)));

        hub.publish("late", 1, grid(1));
        assert_eq!(
            hub.subscribe("late").map(|(kind, _)| kind),
            Some(StreamKind::FusionGrid),
            "a published frame names its own kind"
        );
        assert_eq!(
            hub.subscribe("radar").map(|(kind, _)| kind),
            Some(StreamKind::RangeDoppler)
        );

        hub.forget("tri");
        assert!(hub.subscribe("tri").is_none());
        assert!(matches!(first.try_recv(), Err(TryRecvError::Closed)));
        assert!(hub.subscribe("radar").is_some());
    }

    #[tokio::test]
    async fn a_slow_viewer_learns_how_many_frames_it_missed() {
        let hub = SurfaceHub::default();
        hub.open("tri", StreamKind::FusionGrid);
        let (_, mut slow) = hub.subscribe("tri").expect("surface");
        let sent = u32::try_from(SURFACE_BACKLOG).expect("small backlog") + 3;
        for seq in 0..sent {
            hub.publish("tri", seq, grid(seq));
        }
        assert!(matches!(slow.recv().await, Err(RecvError::Lagged(3))));
        let (seq, _) = slow.recv().await.expect("the newest frames remain");
        assert_eq!(seq, 3);
    }
}
