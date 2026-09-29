use sdrmm_device::{Latency, schedule};
use sdrmm_usb_stream::{NusbBulkIn, RxStream, StreamConfig};

use super::{catalog::Catalog, chip::Chip, demod, error::Result, radio::Dongle, usb::UsbControl};

pub(crate) const TRANSFER_BYTES: usize = 16_384;
const TRANSFERS: usize = 16;
const QUEUED: usize = 32;
pub(crate) const IN_FLIGHT_SAMPLES: u64 = ((TRANSFERS + QUEUED) * TRANSFER_BYTES / 2) as u64;
const BULK_IN: u8 = 0x81;
const PUMP_THREAD: &str = "sdrmm-rtlsdr-usb";

pub(crate) struct Release {
    chip: Chip<UsbControl>,
}

impl Release {
    pub(crate) fn go(&self) -> Result<()> {
        demod::release_endpoint(&self.chip)
    }

    pub(crate) fn rehold(&self) -> Result<()> {
        demod::hold_endpoint(&self.chip)
    }
}

fn config() -> StreamConfig {
    let mut config = StreamConfig::new(TRANSFER_BYTES, PUMP_THREAD);
    config.queue_depth = TRANSFERS;
    config.channel_depth = QUEUED;
    config.on_thread_start = Some(|| schedule::claim(Latency::Critical));
    config
}

impl Dongle {
    pub(crate) fn open(index: usize) -> Result<Self> {
        Catalog::scan()?.open(index)
    }

    pub(crate) fn prime_stream(&mut self) -> Result<(RxStream, Release)> {
        demod::hold_endpoint(self.chip())?;
        let endpoint = NusbBulkIn::open(self.chip().link().interface(), BULK_IN)?;
        let stream = sdrmm_usb_stream::start(endpoint, config())?;
        let release = Release {
            chip: Chip::new(self.chip().link().clone()),
        };
        Ok((stream, release))
    }

    pub(crate) fn start_stream(&mut self) -> Result<RxStream> {
        let (stream, release) = self.prime_stream()?;
        release.go()?;
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_are_whole_packets() {
        assert!(TRANSFER_BYTES.is_multiple_of(512));
        assert_eq!(config().queue_depth, 16);
        assert_eq!(config().transfer_size, 16_384);
    }

    #[test]
    fn a_bank_lane_holds_forty_eight_transfers_in_flight() {
        assert_eq!(IN_FLIGHT_SAMPLES, (16 + 32) * 8_192);
    }
}
