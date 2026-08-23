use crate::{
    CpsError, Image, RadioModel, RadioSession, RegionKind, bits::is_erased, image::changed_blocks,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub step: String,
    pub done: u64,
    pub total: u64,
}

pub trait TransferControl: Send {
    fn cancelled(&self) -> bool {
        false
    }

    fn report(&self, progress: Progress) {
        let _ = progress;
    }
}

#[derive(Default)]
pub struct Silent;

impl TransferControl for Silent {}

pub struct Transfer;

impl Transfer {
    pub fn read(
        model: &dyn RadioModel,
        session: &mut dyn RadioSession,
        control: &dyn TransferControl,
    ) -> Result<Image, CpsError> {
        let total = model.transfer_bytes();
        let block = session.block_size().max(1);
        let mut image = Image::new();
        let mut done = 0u64;

        for region in model.regions() {
            let chunk = chunk_size(region.kind, block);
            let mut empty_run = 0u32;
            let mut offset = 0u32;
            while offset < region.len {
                if control.cancelled() {
                    return Err(CpsError::Cancelled);
                }
                let take = chunk.min(region.len - offset);
                let addr = region.addr + offset;
                image.allocate(addr, take, model.erased_byte());
                let Some(slot) = image.get_mut(addr, take as usize) else {
                    return Err(CpsError::MissingRegion {
                        addr,
                        len: take as usize,
                    });
                };
                session.read(addr, slot)?;
                let empty = is_erased(slot);
                offset += take;
                done += u64::from(take);
                control.report(Progress {
                    step: region.name.to_owned(),
                    done,
                    total,
                });
                if let RegionKind::Sparse {
                    stop_after_empty, ..
                } = region.kind
                {
                    empty_run = if empty { empty_run + 1 } else { 0 };
                    if empty_run >= stop_after_empty {
                        done += u64::from(region.len - offset);
                        break;
                    }
                }
            }
        }

        control.report(Progress {
            step: "done".to_owned(),
            done: total,
            total,
        });
        Ok(image)
    }

    pub fn write(
        model: &dyn RadioModel,
        session: &mut dyn RadioSession,
        before: &Image,
        after: &Image,
        control: &dyn TransferControl,
    ) -> Result<u64, CpsError> {
        let block = session.block_size().max(1);
        let blocks = changed_blocks(before, after, block);
        let total: u64 = blocks.iter().map(|(_, data)| data.len() as u64).sum();
        let mut done = 0u64;
        for (addr, data) in &blocks {
            if control.cancelled() {
                return Err(CpsError::Cancelled);
            }
            session.write(*addr, data)?;
            done += data.len() as u64;
            control.report(Progress {
                step: "writing".to_owned(),
                done,
                total,
            });
        }
        let _ = model;
        Ok(done)
    }
}

fn chunk_size(kind: RegionKind, block: u32) -> u32 {
    match kind {
        RegionKind::Sparse { stride, .. } if stride > 0 => stride.div_ceil(block) * block,
        _ => block,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    };

    use sdrmm_wire::cps::{Codeplug, ConversionReport, RadioIdent, RadioModelDescriptor};

    use super::*;
    use crate::{Region, SerialLink, registry::test_support::demo_descriptor};

    struct StubModel;

    static REGIONS: &[Region] = &[
        Region::fixed("head", 0x0000, 64),
        Region::sparse("body", 0x1000, 512, 64, 2),
    ];

    impl RadioModel for StubModel {
        fn descriptor(&self) -> RadioModelDescriptor {
            demo_descriptor()
        }

        fn regions(&self) -> &'static [Region] {
            REGIONS
        }

        fn open(&self, _link: Box<dyn SerialLink>) -> Result<Box<dyn RadioSession>, CpsError> {
            Err(CpsError::Transport("stub".to_owned()))
        }

        fn decode(&self, _image: &Image) -> Result<Codeplug, CpsError> {
            Ok(Codeplug::empty())
        }

        fn encode(
            &self,
            _codeplug: &Codeplug,
            _image: &mut Image,
        ) -> Result<ConversionReport, CpsError> {
            Err(CpsError::Codeplug("stub".to_owned()))
        }
    }

    struct StubSession {
        reads: Arc<AtomicU32>,
        writes: Arc<AtomicU32>,
    }

    impl RadioSession for StubSession {
        fn identify(&mut self) -> Result<RadioIdent, CpsError> {
            Err(CpsError::Transport("stub".to_owned()))
        }

        fn block_size(&self) -> u32 {
            16
        }

        fn read(&mut self, addr: u32, buffer: &mut [u8]) -> Result<(), CpsError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let filled = if addr < 0x1080 { 0x5a } else { 0 };
            buffer.fill(filled);
            Ok(())
        }

        fn write(&mut self, _addr: u32, _data: &[u8]) -> Result<(), CpsError> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn finish(&mut self) -> Result<(), CpsError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Cancelling(AtomicBool);

    impl TransferControl for Cancelling {
        fn cancelled(&self) -> bool {
            self.0.swap(true, Ordering::Relaxed)
        }
    }

    #[test]
    fn a_sparse_chunk_covers_a_whole_element_even_when_the_stride_is_ragged() {
        assert_eq!(
            chunk_size(
                RegionKind::Sparse {
                    stride: 200,
                    stop_after_empty: 4
                },
                16
            ),
            208
        );
        assert_eq!(
            chunk_size(
                RegionKind::Sparse {
                    stride: 128,
                    stop_after_empty: 4
                },
                16
            ),
            128
        );
        assert_eq!(chunk_size(RegionKind::Fixed, 1024), 1024);
    }

    #[test]
    fn a_sparse_region_stops_after_the_configured_run_of_empty_elements() {
        let reads = Arc::new(AtomicU32::new(0));
        let mut session = StubSession {
            reads: reads.clone(),
            writes: Arc::new(AtomicU32::new(0)),
        };
        let image = Transfer::read(&StubModel, &mut session, &Silent).expect("read");
        assert_eq!(image.get(0x0000, 64), Some([0x5a; 64].as_slice()));
        assert_eq!(reads.load(Ordering::Relaxed), 4 + 4);
        assert_eq!(image.get(0x1000, 128).map(<[u8]>::len), Some(128));
        assert_eq!(image.get(0x11c0, 8), None);
    }

    #[test]
    fn only_changed_blocks_reach_the_radio() {
        let writes = Arc::new(AtomicU32::new(0));
        let mut session = StubSession {
            reads: Arc::new(AtomicU32::new(0)),
            writes: writes.clone(),
        };
        let mut before = Image::new();
        before.allocate(0, 64, 0);
        let mut after = before.clone();
        if let Some(slot) = after.get_mut(32, 1) {
            slot[0] = 1;
        }
        let sent =
            Transfer::write(&StubModel, &mut session, &before, &after, &Silent).expect("write");
        assert_eq!(sent, 16);
        assert_eq!(writes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_cancelled_transfer_stops_instead_of_finishing() {
        let mut session = StubSession {
            reads: Arc::new(AtomicU32::new(0)),
            writes: Arc::new(AtomicU32::new(0)),
        };
        let error = Transfer::read(&StubModel, &mut session, &Cancelling::default())
            .expect_err("cancelled");
        assert!(error.is_cancelled(), "{error}");
    }
}
