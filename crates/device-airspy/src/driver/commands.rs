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
    fn request_numbers_match_the_firmware_table() {
        assert_eq!(VendorRequest::ReceiverMode as u8, 1);
        assert_eq!(VendorRequest::SetSampleRate as u8, 12);
        assert_eq!(VendorRequest::SetFreq as u8, 13);
        assert_eq!(VendorRequest::GetSampleRates as u8, 25);
        assert_eq!(VendorRequest::SetPacking as u8, 26);
    }
}
