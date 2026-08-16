#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum BoardId {
    Jellybean,
    Jawbreaker,
    HackRfOne,
    Rad1o,
    HackRfOneR9,
    HackRfPro,
    Unrecognized,
    Undetected,
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

    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceInfo {
    pub(crate) board_id: BoardId,
    pub(crate) firmware_version: String,
    pub(crate) usb_api_version: u16,
    pub(crate) serial: Option<u128>,
}

impl DeviceInfo {
    #[must_use]
    pub(crate) const fn board_name(&self) -> &'static str {
        self.board_id.name()
    }
}

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
