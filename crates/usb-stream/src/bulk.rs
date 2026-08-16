use std::{ops::Deref, time::Duration};

use nusb::{
    Endpoint, MaybeFuture,
    transfer::{Buffer, Bulk, In, TransferError},
};

use crate::error::Result;

#[derive(Debug)]
pub struct Completion<B> {
    pub buffer: B,
    pub actual_len: usize,
    pub status: std::result::Result<(), TransferError>,
}

pub trait BulkIn: Send + 'static {
    type Buffer: Deref<Target = [u8]> + Send;

    fn clear_halt(&mut self) -> Result<()>;
    fn max_packet_size(&self) -> usize;
    fn allocate(&self, len: usize) -> Self::Buffer;
    fn submit(&mut self, buffer: Self::Buffer);
    fn pending(&self) -> usize;
    fn wait_next_complete(&mut self, timeout: Duration) -> Option<Completion<Self::Buffer>>;
    fn cancel_all(&mut self);
}

#[derive(Debug)]
pub struct NusbBulkIn {
    endpoint: Endpoint<Bulk, In>,
}

impl NusbBulkIn {
    pub fn open(interface: &nusb::Interface, address: u8) -> Result<Self> {
        Ok(Self {
            endpoint: interface.endpoint::<Bulk, In>(address)?,
        })
    }
}

impl BulkIn for NusbBulkIn {
    type Buffer = Buffer;

    fn clear_halt(&mut self) -> Result<()> {
        self.endpoint.clear_halt().wait()?;
        Ok(())
    }

    fn max_packet_size(&self) -> usize {
        self.endpoint.max_packet_size()
    }

    fn allocate(&self, len: usize) -> Self::Buffer {
        self.endpoint.allocate(len)
    }

    fn submit(&mut self, buffer: Self::Buffer) {
        self.endpoint.submit(buffer);
    }

    fn pending(&self) -> usize {
        self.endpoint.pending()
    }

    fn wait_next_complete(&mut self, timeout: Duration) -> Option<Completion<Self::Buffer>> {
        self.endpoint
            .wait_next_complete(timeout)
            .map(|completion| Completion {
                buffer: completion.buffer,
                actual_len: completion.actual_len,
                status: completion.status,
            })
    }

    fn cancel_all(&mut self) {
        self.endpoint.cancel_all();
    }
}
