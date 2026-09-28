#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum VendorRequest {
    ReceiverMode = 1,
    VersionStringRead = 10,
    BoardPartIdSerialNoRead = 11,
    SetSampleRate = 12,
    SetFreq = 13,
    SetLnaGain = 14,
    SetMixerGain = 15,
    SetVgaGain = 16,
    SetLnaAgc = 17,
    SetMixerAgc = 18,
    GpioWrite = 21,
    GetSampleRates = 25,
    SetPacking = 26,
}

impl VendorRequest {
    pub(crate) const fn accepted(self, status: u8) -> bool {
        match self {
            Self::SetSampleRate | Self::SetPacking => status != 0,
            _ => status == 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub(crate) enum ReceiverMode {
    Off = 0,
    Rx = 1,
}

/// GPIO port 1 pin 13 carries the bias-tee switch, and the firmware takes the two packed into
/// one index rather than as separate fields.
const BIAS_TEE_PORT: u16 = 1;
const BIAS_TEE_PIN: u16 = 13;

pub(crate) const fn bias_tee_port_pin() -> u16 {
    (BIAS_TEE_PORT << 5) | BIAS_TEE_PIN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bias_tee_pin_packs_into_one_index() {
        assert_eq!(bias_tee_port_pin(), 45);
    }

    #[test]
    fn rate_and_packing_answer_one_when_accepted() {
        assert!(VendorRequest::SetSampleRate.accepted(1));
        assert!(!VendorRequest::SetSampleRate.accepted(0));
        assert!(VendorRequest::SetPacking.accepted(1));
        assert!(!VendorRequest::SetPacking.accepted(0));
    }

    #[test]
    fn tuner_requests_answer_zero_when_accepted() {
        for request in [
            VendorRequest::SetLnaGain,
            VendorRequest::SetMixerGain,
            VendorRequest::SetVgaGain,
            VendorRequest::SetLnaAgc,
            VendorRequest::SetMixerAgc,
        ] {
            assert!(request.accepted(0), "{request:?}");
            assert!(!request.accepted(0xff), "{request:?}");
        }
    }

    #[test]
    fn request_numbers_match_the_firmware_table() {
        assert_eq!(VendorRequest::ReceiverMode as u8, 1);
        assert_eq!(VendorRequest::SetSampleRate as u8, 12);
        assert_eq!(VendorRequest::SetFreq as u8, 13);
        assert_eq!(VendorRequest::GetSampleRates as u8, 25);
        assert_eq!(VendorRequest::SetPacking as u8, 26);
    }
}
