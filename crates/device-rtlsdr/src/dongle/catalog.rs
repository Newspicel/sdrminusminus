use nusb::MaybeFuture;

use super::{
    board::Board,
    error::{Error, Result},
    radio::Dongle,
    usb::UsbControl,
};

const RTL_IDS: [(u16, u16); 43] = [
    (0x0bda, 0x2832),
    (0x0bda, 0x2838),
    (0x0413, 0x6680),
    (0x0413, 0x6f0f),
    (0x0458, 0x707f),
    (0x0ccd, 0x00a9),
    (0x0ccd, 0x00b3),
    (0x0ccd, 0x00b4),
    (0x0ccd, 0x00b5),
    (0x0ccd, 0x00b7),
    (0x0ccd, 0x00b8),
    (0x0ccd, 0x00b9),
    (0x0ccd, 0x00c0),
    (0x0ccd, 0x00c6),
    (0x0ccd, 0x00d3),
    (0x0ccd, 0x00d7),
    (0x0ccd, 0x00e0),
    (0x1554, 0x5020),
    (0x15f4, 0x0131),
    (0x15f4, 0x0133),
    (0x185b, 0x0620),
    (0x185b, 0x0650),
    (0x185b, 0x0680),
    (0x1b80, 0xd393),
    (0x1b80, 0xd394),
    (0x1b80, 0xd395),
    (0x1b80, 0xd397),
    (0x1b80, 0xd398),
    (0x1b80, 0xd39d),
    (0x1b80, 0xd3a4),
    (0x1b80, 0xd3a8),
    (0x1b80, 0xd3af),
    (0x1b80, 0xd3b0),
    (0x1d19, 0x1101),
    (0x1d19, 0x1102),
    (0x1d19, 0x1103),
    (0x1d19, 0x1104),
    (0x1f4d, 0xa803),
    (0x1f4d, 0xb803),
    (0x1f4d, 0xc803),
    (0x1f4d, 0xd286),
    (0x1f4d, 0xd803),
    (0x1209, 0x2832),
];

pub(crate) fn is_rtl(vendor: u16, product: u16) -> bool {
    RTL_IDS.contains(&(vendor, product))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) index: usize,
    pub(crate) bus: String,
    pub(crate) address: u8,
    pub(crate) manufacturer: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) port_chain: Vec<u8>,
    pub(crate) board: Board,
}

impl Listing {
    fn from_usb(index: usize, info: &nusb::DeviceInfo) -> Self {
        Self {
            index,
            bus: info.bus_id().to_owned(),
            address: info.device_address(),
            manufacturer: info.manufacturer_string().map(str::to_owned),
            product: info.product_string().map(str::to_owned),
            serial: info.serial_number().map(str::to_owned),
            port_chain: info.port_chain().to_vec(),
            board: Board::detect(info.manufacturer_string(), info.product_string()),
        }
    }
}

pub(crate) struct Catalog {
    found: Vec<(Listing, nusb::DeviceInfo)>,
}

impl Catalog {
    pub(crate) fn scan() -> Result<Self> {
        let found = nusb::list_devices()
            .wait()
            .map_err(Error::Scan)?
            .filter(|info| is_rtl(info.vendor_id(), info.product_id()))
            .enumerate()
            .map(|(index, info)| (Listing::from_usb(index, &info), info))
            .collect();
        Ok(Self { found })
    }

    pub(crate) fn listings(&self) -> impl ExactSizeIterator<Item = &Listing> {
        self.found.iter().map(|(listing, _)| listing)
    }

    pub(crate) fn open(&self, index: usize) -> Result<Dongle> {
        let (listing, info) = self.found.get(index).ok_or(Error::NotFound(index))?;
        tracing::info!(
            bus = %listing.bus,
            address = listing.address,
            product = ?listing.product,
            board = ?listing.board,
            "opening RTL-SDR"
        );
        Dongle::start(UsbControl::open(info)?, listing.board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_pair_is_an_rtl_sdr() {
        assert_eq!(RTL_IDS.len(), 43);
        for (vendor, product) in RTL_IDS {
            assert!(is_rtl(vendor, product));
        }
    }

    #[test]
    fn near_misses_are_not() {
        assert!(!is_rtl(0x0bda, 0x2839));
        assert!(!is_rtl(0x1d50, 0x6089));
    }
}
