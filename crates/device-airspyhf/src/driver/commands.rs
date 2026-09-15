#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum VendorRequest {
    ReceiverMode = 1,
    SetFreq = 2,
    GetSampleRates = 3,
    SetSampleRate = 4,
    GetSerialNoBoardId = 7,
    GetVersionString = 9,
    SetAgc = 10,
    SetAgcThreshold = 11,
    SetAtt = 12,
    SetLna = 13,
    GetSampleRateArchitectures = 14,
    SetBiasTee = 22,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub(crate) enum ReceiverMode {
    Off = 0,
    On = 1,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_numbers_match_the_firmware_table() {
        assert_eq!(VendorRequest::ReceiverMode as u8, 1);
        assert_eq!(VendorRequest::SetFreq as u8, 2);
        assert_eq!(VendorRequest::GetSampleRates as u8, 3);
        assert_eq!(VendorRequest::SetSampleRate as u8, 4);
        assert_eq!(VendorRequest::GetSerialNoBoardId as u8, 7);
        assert_eq!(VendorRequest::GetVersionString as u8, 9);
        assert_eq!(VendorRequest::SetAtt as u8, 12);
        assert_eq!(VendorRequest::SetBiasTee as u8, 22);
    }
}
