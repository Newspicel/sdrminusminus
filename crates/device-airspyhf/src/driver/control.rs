use std::time::Duration;

use nusb::{
    Interface, MaybeFuture,
    transfer::{ControlIn, ControlOut, ControlType, Recipient},
};

use super::{
    commands::{ReceiverMode, VendorRequest},
    error::{Error, Result},
};

const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);
pub(crate) const VERSION_STRING_SIZE: usize = 255;
pub(crate) const PART_ID_SERIAL_SIZE: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    In,
    Out,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VendorControlRequest {
    direction: Direction,
    request: VendorRequest,
    value: u16,
    index: u16,
    length: usize,
    data: Vec<u8>,
}

impl VendorControlRequest {
    fn in_request(request: VendorRequest, value: u16, index: u16, length: usize) -> Self {
        Self {
            direction: Direction::In,
            request,
            value,
            index,
            length,
            data: Vec::new(),
        }
    }

    fn out_request(request: VendorRequest, value: u16, index: u16, data: Vec<u8>) -> Self {
        Self {
            direction: Direction::Out,
            request,
            value,
            index,
            length: data.len(),
            data,
        }
    }

    pub(crate) fn receiver_mode(mode: ReceiverMode) -> Self {
        Self::out_request(VendorRequest::ReceiverMode, mode as u16, 0, Vec::new())
    }

    /// The radio tunes in whole kilohertz, and takes them most significant byte first: the one
    /// big-endian field in this protocol.
    pub(crate) fn set_frequency(frequency_khz: u32) -> Self {
        Self::out_request(
            VendorRequest::SetFreq,
            0,
            0,
            frequency_khz.to_be_bytes().to_vec(),
        )
    }

    pub(crate) fn set_sample_rate_index(index: u16) -> Self {
        Self::out_request(VendorRequest::SetSampleRate, 0, index, Vec::new())
    }

    pub(crate) fn sample_rate_count() -> Self {
        Self::in_request(VendorRequest::GetSampleRates, 0, 0, 4)
    }

    pub(crate) fn sample_rates(count: u16) -> Self {
        Self::in_request(
            VendorRequest::GetSampleRates,
            0,
            count,
            usize::from(count) * 4,
        )
    }

    pub(crate) fn sample_rate_architectures(count: u16) -> Self {
        Self::in_request(
            VendorRequest::GetSampleRateArchitectures,
            0,
            count,
            usize::from(count),
        )
    }

    pub(crate) fn set_agc(enabled: bool) -> Self {
        Self::out_request(VendorRequest::SetAgc, u16::from(enabled), 0, Vec::new())
    }

    pub(crate) fn set_agc_threshold(high: bool) -> Self {
        Self::out_request(
            VendorRequest::SetAgcThreshold,
            u16::from(high),
            0,
            Vec::new(),
        )
    }

    pub(crate) fn set_attenuation(step: u8) -> Self {
        Self::out_request(VendorRequest::SetAtt, u16::from(step), 0, Vec::new())
    }

    pub(crate) fn set_lna(enabled: bool) -> Self {
        Self::out_request(VendorRequest::SetLna, u16::from(enabled), 0, Vec::new())
    }

    pub(crate) fn set_bias_tee(enabled: bool) -> Self {
        Self::out_request(VendorRequest::SetBiasTee, u16::from(enabled), 0, Vec::new())
    }

    pub(crate) fn version_string_read() -> Self {
        Self::in_request(VendorRequest::GetVersionString, 0, 0, VERSION_STRING_SIZE)
    }

    pub(crate) fn part_id_serial_read() -> Self {
        Self::in_request(VendorRequest::GetSerialNoBoardId, 0, 0, PART_ID_SERIAL_SIZE)
    }
}

#[derive(Debug)]
pub(crate) struct Control {
    _device: nusb::Device,
    interface: Interface,
}

impl Control {
    pub(crate) fn new(device: nusb::Device, interface: Interface) -> Self {
        Self {
            _device: device,
            interface,
        }
    }

    pub(crate) fn interface(&self) -> &Interface {
        &self.interface
    }

    pub(crate) fn control_in(&self, request: &VendorControlRequest) -> Result<Vec<u8>> {
        debug_assert_eq!(request.direction, Direction::In);
        let length = u16::try_from(request.length).map_err(|_| {
            Error::protocol(
                "encode control IN request",
                "response length exceeds 64 KiB",
            )
        })?;
        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: request.request as u8,
                    value: request.value,
                    index: request.index,
                    length,
                },
                CONTROL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)
    }

    pub(crate) fn control_out(&self, request: &VendorControlRequest) -> Result<()> {
        debug_assert_eq!(request.direction, Direction::Out);
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: request.request as u8,
                    value: request.value,
                    index: request.index,
                    data: &request.data,
                },
                CONTROL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)
    }

    pub(crate) fn control_in_exact(
        &self,
        request: &VendorControlRequest,
        expected: usize,
    ) -> Result<Vec<u8>> {
        let response = self.control_in(request)?;
        if response.len() != expected {
            return Err(Error::protocol(
                "read control response",
                "the radio answered with the wrong number of bytes",
            ));
        }
        Ok(response)
    }
}

pub(crate) fn decode_c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// The reply is a part id followed by four serial words, of which the last two carry the number
/// the radio is known by.
pub(crate) fn decode_serial(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < PART_ID_SERIAL_SIZE {
        return None;
    }
    let high = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    let low = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    Some((u64::from(high) << 32) | u64::from(low))
}

pub(crate) fn decode_sample_rates(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .filter(|rate| *rate > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frequency_travels_as_kilohertz_most_significant_byte_first() {
        let request = VendorControlRequest::set_frequency(14_200);
        assert_eq!(request.data, vec![0x00, 0x00, 0x37, 0x78]);
        assert_eq!(u32::from_be_bytes([0, 0, 0x37, 0x78]), 14_200);
        assert_eq!(request.value, 0);
        assert_eq!(request.index, 0);
    }

    #[test]
    fn a_rate_index_travels_in_the_index_with_no_payload() {
        let request = VendorControlRequest::set_sample_rate_index(2);
        assert_eq!(request.index, 2);
        assert!(request.data.is_empty());
    }

    #[test]
    fn switches_travel_in_the_value() {
        assert_eq!(VendorControlRequest::set_agc(true).value, 1);
        assert_eq!(VendorControlRequest::set_lna(false).value, 0);
        assert_eq!(VendorControlRequest::set_bias_tee(true).value, 1);
        assert_eq!(VendorControlRequest::set_attenuation(6).value, 6);
        assert_eq!(VendorControlRequest::set_agc_threshold(true).value, 1);
    }

    #[test]
    fn the_rate_list_is_asked_for_in_two_steps() {
        assert_eq!(VendorControlRequest::sample_rate_count().length, 4);
        assert_eq!(VendorControlRequest::sample_rates(4).index, 4);
        assert_eq!(VendorControlRequest::sample_rates(4).length, 16);
        assert_eq!(VendorControlRequest::sample_rate_architectures(4).length, 4);
    }

    #[test]
    fn a_serial_takes_the_last_two_words_of_the_part_id_reply() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x0000_6906_u32.to_le_bytes());
        bytes.extend_from_slice(&0x1111_1111_u32.to_le_bytes());
        bytes.extend_from_slice(&0x2222_2222_u32.to_le_bytes());
        bytes.extend_from_slice(&0x3b2d_4b8b_u32.to_le_bytes());
        bytes.extend_from_slice(&0x675c_62dc_u32.to_le_bytes());
        assert_eq!(decode_serial(&bytes), Some(0x3b2d_4b8b_675c_62dc));
        assert_eq!(decode_serial(&bytes[..16]), None);
    }

    #[test]
    fn a_version_string_stops_at_its_terminator() {
        let mut bytes = b"AirSpy HF+ v1.6.8\0".to_vec();
        bytes.extend_from_slice(&[0xff; 8]);
        assert_eq!(decode_c_string(&bytes), "AirSpy HF+ v1.6.8");
    }

    #[test]
    fn a_rate_list_drops_the_zeroes_a_short_firmware_pads_with() {
        let mut bytes = Vec::new();
        for rate in [768_000_u32, 384_000, 256_000, 0] {
            bytes.extend_from_slice(&rate.to_le_bytes());
        }
        assert_eq!(decode_sample_rates(&bytes), vec![768_000, 384_000, 256_000]);
    }
}
