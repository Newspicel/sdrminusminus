//! Board identity and the metadata collected while opening a radio.

/// Board identity as the firmware reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BoardId {
    /// Pre-production Jellybean board.
    Jellybean,
    /// HackRF Jawbreaker beta board.
    Jawbreaker,
    /// Original HackRF One board.
    HackRfOne,
    /// rad1o badge variant.
    Rad1o,
    /// HackRF One revision 9 or later.
    HackRfOneR9,
    /// HackRF Pro / Praline platform.
    HackRfPro,
    /// Firmware explicitly reported an unrecognised board.
    Unrecognized,
    /// Firmware has not detected a board.
    Undetected,
    /// A board ID from firmware newer than this driver.
    Unknown(u8),
}

impl BoardId {
    pub(crate) const fn from_raw(value: u8) -> Self {
        match value {
            0 => Self::Jellybean,
            1 => Self::Jawbreaker,
            2 => Self::HackRfOne,
            3 => Self::Rad1o,
            4 => Self::HackRfOneR9,
            5 => Self::HackRfPro,
            0xfe => Self::Unrecognized,
            0xff => Self::Undetected,
            other => Self::Unknown(other),
        }
    }

    /// Board name in libhackrf's terminology.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Jellybean => "Jellybean",
            Self::Jawbreaker => "Jawbreaker",
            Self::HackRfOne | Self::HackRfOneR9 => "HackRF One",
            Self::Rad1o => "rad1o",
            Self::HackRfPro => "HackRF Pro",
            Self::Unrecognized => "unrecognized",
            Self::Undetected => "undetected",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// What the firmware said about itself while the device was being opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    /// Firmware-reported board identity.
    pub board_id: BoardId,
    /// Firmware version string.
    pub firmware_version: String,
    /// HackRF USB API version from the USB `bcdDevice` field: high byte major, low byte minor,
    /// read as hexadecimal `MM.mm` exactly as libhackrf does.
    pub usb_api_version: u16,
    /// The MCU's 128-bit serial, when it is nonzero.
    pub serial: Option<u128>,
}

impl DeviceInfo {
    /// Board name for display.
    #[must_use]
    pub const fn board_name(&self) -> &'static str {
        self.board_id.name()
    }
}

/// The part-ID and serial words as `BoardPartIdSerialNoRead` returns them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PartIdSerial {
    pub(crate) serial: [u32; 4],
}

impl PartIdSerial {
    pub(crate) const fn serial_u128(self) -> Option<u128> {
        let [a, b, c, d] = self.serial;
        let value = ((a as u128) << 96) | ((b as u128) << 64) | ((c as u128) << 32) | d as u128;
        if value == 0 { None } else { Some(value) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_ids_follow_libhackrf_numbering() {
        assert_eq!(BoardId::from_raw(2), BoardId::HackRfOne);
        assert_eq!(BoardId::from_raw(4).name(), "HackRF One");
        assert_eq!(BoardId::from_raw(0xff), BoardId::Undetected);
        assert_eq!(BoardId::from_raw(7), BoardId::Unknown(7));
    }

    #[test]
    fn an_all_zero_serial_is_no_serial() {
        assert_eq!(PartIdSerial { serial: [0; 4] }.serial_u128(), None);
        assert_eq!(
            PartIdSerial {
                serial: [0, 0, 0x675c_62dc, 0x3b2d_4b8b],
            }
            .serial_u128(),
            Some(0x675c_62dc_3b2d_4b8b)
        );
    }
}
