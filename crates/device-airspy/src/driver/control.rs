use std::time::Duration;

use nusb::{
    Interface, MaybeFuture,
    transfer::{ControlIn, ControlOut, ControlType, Recipient},
};

use super::{
    commands::{ReceiverMode, VendorRequest, bias_tee_port_pin},
    error::{Error, Result},
};

const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);
pub(crate) const VERSION_STRING_SIZE: usize = 127;
pub(crate) const PART_ID_SERIAL_SIZE: usize = 16;

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

    pub(crate) fn set_frequency(frequency_hz: u32) -> Self {
        Self::out_request(
            VendorRequest::SetFreq,
            0,
            0,
            frequency_hz.to_le_bytes().to_vec(),
        )
    }

    /// The firmware takes the position of a rate in the list it published, never the rate itself.
    pub(crate) fn set_sample_rate_index(index: u16) -> Self {
        Self::in_request(VendorRequest::SetSampleRate, 0, index, 1)
    }

    /// Asked for nothing, the firmware answers with how many rates it has; asked for that many,
    /// it answers with the rates.
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

    pub(crate) fn set_packing(enabled: bool) -> Self {
        Self::in_request(VendorRequest::SetPacking, 0, u16::from(enabled), 1)
    }

    pub(crate) fn set_lna_gain(gain: u8) -> Self {
        Self::in_request(VendorRequest::SetLnaGain, 0, u16::from(gain), 1)
    }

    pub(crate) fn set_mixer_gain(gain: u8) -> Self {
        Self::in_request(VendorRequest::SetMixerGain, 0, u16::from(gain), 1)
    }

    pub(crate) fn set_vga_gain(gain: u8) -> Self {
        Self::in_request(VendorRequest::SetVgaGain, 0, u16::from(gain), 1)
    }

    pub(crate) fn set_lna_agc(enabled: bool) -> Self {
        Self::in_request(VendorRequest::SetLnaAgc, 0, u16::from(enabled), 1)
    }

    pub(crate) fn set_mixer_agc(enabled: bool) -> Self {
        Self::in_request(VendorRequest::SetMixerAgc, 0, u16::from(enabled), 1)
    }

    pub(crate) fn set_bias_tee(enabled: bool) -> Self {
        Self::out_request(
            VendorRequest::GpioWrite,
            u16::from(enabled),
            bias_tee_port_pin(),
            Vec::new(),
        )
    }

    pub(crate) fn version_string_read() -> Self {
        Self::in_request(VendorRequest::VersionStringRead, 0, 0, VERSION_STRING_SIZE)
    }

    pub(crate) fn part_id_serial_read() -> Self {
        Self::in_request(
            VendorRequest::BoardPartIdSerialNoRead,
            0,
            0,
            PART_ID_SERIAL_SIZE,
        )
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

    /// Runs a request the firmware answers with a status byte, which it sets to zero when it
    /// rejected the value. A refused gain that read as success would leave the reported settings
    /// describing a radio that is not configured that way.
    pub(crate) fn control_in_accepted(
        &self,
        request: &VendorControlRequest,
        operation: &'static str,
    ) -> Result<()> {
        let response = self.control_in(request)?;
        match response.first() {
            Some(0) | None => Err(Error::protocol(operation, "the radio refused the value")),
            Some(_) => Ok(()),
        }
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

/// The last two of the four serial words identify the board; libairspy prints the pair, so a
/// radio's label here is the one its own tools show.
pub(crate) fn decode_serial(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < PART_ID_SERIAL_SIZE {
        return None;
    }
    let high = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let low = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
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
    fn a_frequency_travels_as_four_little_endian_bytes() {
        let request = VendorControlRequest::set_frequency(100_000_000);
        assert_eq!(request.data, 100_000_000_u32.to_le_bytes());
        assert_eq!(request.value, 0);
        assert_eq!(request.index, 0);
    }

    #[test]
    fn gains_travel_in_the_index_not_the_value() {
        assert_eq!(VendorControlRequest::set_lna_gain(7).index, 7);
        assert_eq!(VendorControlRequest::set_mixer_gain(9).index, 9);
        assert_eq!(VendorControlRequest::set_vga_gain(12).index, 12);
        assert_eq!(VendorControlRequest::set_lna_gain(7).value, 0);
    }

    #[test]
    fn the_bias_tee_switch_travels_in_the_value_and_names_its_pin_in_the_index() {
        let on = VendorControlRequest::set_bias_tee(true);
        assert_eq!(on.value, 1);
        assert_eq!(on.index, 45);
        assert_eq!(VendorControlRequest::set_bias_tee(false).value, 0);
    }

    #[test]
    fn the_rate_list_is_asked_for_in_two_steps() {
        assert_eq!(VendorControlRequest::sample_rate_count().index, 0);
        assert_eq!(VendorControlRequest::sample_rate_count().length, 4);
        let list = VendorControlRequest::sample_rates(3);
        assert_eq!(list.index, 3);
        assert_eq!(list.length, 12);
    }

    #[test]
    fn a_version_string_stops_at_its_terminator() {
        let mut bytes = b"AirSpy NOS v1.0.0-rc10-0-g946184a 2020-05-08\0".to_vec();
        bytes.extend_from_slice(&[0xff; 8]);
        assert_eq!(
            decode_c_string(&bytes),
            "AirSpy NOS v1.0.0-rc10-0-g946184a 2020-05-08"
        );
    }

    #[test]
    fn a_serial_takes_the_last_two_words_of_the_part_id_reply() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x1111_1111_u32.to_le_bytes());
        bytes.extend_from_slice(&0x2222_2222_u32.to_le_bytes());
        bytes.extend_from_slice(&0x6447_0000_u32.to_le_bytes());
        bytes.extend_from_slice(&0x2e19_a5b3_u32.to_le_bytes());
        assert_eq!(decode_serial(&bytes), Some(0x6447_0000_2e19_a5b3));
        assert_eq!(decode_serial(&bytes[..8]), None);
    }

    #[test]
    fn a_rate_list_drops_the_zeroes_a_short_firmware_pads_with() {
        let mut bytes = Vec::new();
        for rate in [10_000_000_u32, 2_500_000, 0] {
            bytes.extend_from_slice(&rate.to_le_bytes());
        }
        assert_eq!(decode_sample_rates(&bytes), vec![10_000_000, 2_500_000]);
    }
}
