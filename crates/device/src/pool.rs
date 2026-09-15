use std::{
    ops::Deref,
    sync::{Arc, Mutex},
};

use crate::lock;

const KEPT: usize = 4;

/// Recycles the byte buffers a capture path fills, so a running stream allocates nothing.
#[derive(Clone, Debug, Default)]
pub struct BlockPool {
    free: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl BlockPool {
    #[must_use]
    pub fn take(&self, len: usize) -> Block {
        let mut bytes = lock(&self.free).pop().unwrap_or_default();
        bytes.clear();
        bytes.resize(len, 0);
        Block {
            bytes,
            pool: self.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Block {
    bytes: Vec<u8>,
    pool: BlockPool,
}

impl Block {
    pub fn truncate(&mut self, len: usize) {
        self.bytes.truncate(len);
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

impl Deref for Block {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        let mut free = lock(&self.pool.free);
        if free.len() < KEPT {
            free.push(std::mem::take(&mut self.bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_come_back_to_the_pool_and_are_reused() {
        let pool = BlockPool::default();
        let address = {
            let mut block = pool.take(64);
            assert_eq!(block.len(), 64);
            block.truncate(8);
            assert_eq!(block.len(), 8);
            block.as_ptr()
        };
        let block = pool.take(64);
        assert_eq!(
            block.as_ptr(),
            address,
            "the capture path must not allocate"
        );
        assert_eq!(block.len(), 64, "a reused block is resized back to full");
    }

    #[test]
    fn the_pool_stops_hoarding_beyond_what_a_stream_needs() {
        let pool = BlockPool::default();
        let blocks: Vec<Block> = (0..KEPT + 3).map(|_| pool.take(8)).collect();
        drop(blocks);
        assert_eq!(lock(&pool.free).len(), KEPT);
    }
}
