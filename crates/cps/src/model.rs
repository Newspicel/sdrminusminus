use sdrmm_wire::cps::{Codeplug, ConversionReport, RadioIdent, RadioLimits, RadioModelDescriptor};

use crate::{CpsError, Image, Region, SerialLink};

pub trait RadioSession: Send {
    fn identify(&mut self) -> Result<RadioIdent, CpsError>;
    fn block_size(&self) -> u32;
    fn read(&mut self, addr: u32, buffer: &mut [u8]) -> Result<(), CpsError>;
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), CpsError>;
    fn finish(&mut self) -> Result<(), CpsError>;
}

pub trait RadioModel: Send + Sync {
    fn descriptor(&self) -> RadioModelDescriptor;

    fn limits(&self) -> RadioLimits {
        self.descriptor().limits
    }

    fn regions(&self) -> &'static [Region];

    fn baud(&self) -> u32 {
        115_200
    }

    fn open(&self, link: Box<dyn SerialLink>) -> Result<Box<dyn RadioSession>, CpsError>;

    fn blank_image(&self) -> Image {
        let mut image = Image::new();
        for region in self.regions() {
            image.allocate(region.addr, region.len, self.erased_byte());
        }
        image
    }

    fn erased_byte(&self) -> u8 {
        0x00
    }

    fn decode(&self, image: &Image) -> Result<Codeplug, CpsError>;

    fn encode(&self, codeplug: &Codeplug, image: &mut Image) -> Result<ConversionReport, CpsError>;

    fn transfer_bytes(&self) -> u64 {
        self.regions()
            .iter()
            .map(|region| u64::from(region.len))
            .sum()
    }
}
