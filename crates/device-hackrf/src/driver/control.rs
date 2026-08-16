use std::time::Duration;

use nusb::{
    Interface, MaybeFuture,
    transfer::{ControlIn, ControlOut, ControlType, Recipient},
};

use super::{
    commands::{TransceiverMode, VendorRequest},
    error::{Error, Result},
    types::PartIdSerial,
};

const CONTROL_TIMEOUT: Duration = Duration::from_millis(100);
const VERSION_STRING_SIZE: usize = 255;

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

    pub(crate) fn transceiver_mode(mode: TransceiverMode) -> Self {
        Self::out_request(
            VendorRequest::SetTransceiverMode,
            mode as u16,
            0,
            Vec::new(),
        )
    }

    pub(crate) fn set_frequency(frequency_hz: u64) -> Self {
        let mhz = (frequency_hz / 1_000_000) as u32;
        let remainder = (frequency_hz % 1_000_000) as u32;
        let mut data = Vec::with_capacity(8);
        data.extend_from_slice(&mhz.to_le_bytes());
        data.extend_from_slice(&remainder.to_le_bytes());
        Self::out_request(VendorRequest::SetFreq, 0, 0, data)
    }

    pub(crate) fn set_sample_rate(sample_rate_hz: u32) -> Self {
        let mut data = Vec::with_capacity(8);
        data.extend_from_slice(&sample_rate_hz.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        Self::out_request(VendorRequest::SampleRateSet, 0, 0, data)
    }

    pub(crate) fn set_baseband_bandwidth(bandwidth_hz: u32) -> Self {
        Self::out_request(
            VendorRequest::BasebandFilterBandwidthSet,
            bandwidth_hz as u16,
            (bandwidth_hz >> 16) as u16,
            Vec::new(),
        )
    }

    pub(crate) fn init_sweep(bytes_per_tuning: u32, payload: Vec<u8>) -> Self {
        Self::out_request(
            VendorRequest::InitSweep,
            bytes_per_tuning as u16,
            (bytes_per_tuning >> 16) as u16,
            payload,
        )
    }

    pub(crate) fn set_lna_gain(gain_db: u8) -> Self {
        Self::in_request(VendorRequest::SetLnaGain, 0, u16::from(gain_db), 1)
    }

    pub(crate) fn set_vga_gain(gain_db: u8) -> Self {
        Self::in_request(VendorRequest::SetVgaGain, 0, u16::from(gain_db), 1)
    }

    pub(crate) fn set_tx_vga_gain(gain_db: u8) -> Self {
        Self::in_request(VendorRequest::SetTxVgaGain, 0, u16::from(gain_db), 1)
    }

    pub(crate) fn get_buffer_size() -> Self {
        Self::in_request(VendorRequest::GetBufferSize, 0, 0, 4)
    }

    pub(crate) fn set_amp(enabled: bool) -> Self {
        Self::out_request(VendorRequest::AmpEnable, enabled.into(), 0, Vec::new())
    }

    pub(crate) fn set_bias_tee(enabled: bool) -> Self {
        Self::out_request(VendorRequest::AntennaEnable, enabled.into(), 0, Vec::new())
    }

    pub(crate) fn board_id_read() -> Self {
        Self::in_request(VendorRequest::BoardIdRead, 0, 0, 1)
    }

    pub(crate) fn version_string_read() -> Self {
        Self::in_request(VendorRequest::VersionStringRead, 0, 0, VERSION_STRING_SIZE)
    }

    pub(crate) fn part_id_serial_read() -> Self {
        Self::in_request(VendorRequest::BoardPartIdSerialNoRead, 0, 0, 24)
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
        let bytes = self.control_in(request)?;
        if bytes.len() == expected {
            Ok(bytes)
        } else {
            Err(Error::protocol(
                "read control response",
                "response has an unexpected length",
            ))
        }
    }
}

pub(crate) fn validate_gain_response(bytes: &[u8], operation: &'static str) -> Result<()> {
    if bytes == [1] {
        Ok(())
    } else {
        Err(Error::protocol(
            operation,
            "firmware rejected the gain value",
        ))
    }
}

pub(crate) fn decode_part_id_serial(bytes: &[u8]) -> Result<PartIdSerial> {
    let (chunks, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() || chunks.len() != 6 {
        return Err(Error::protocol(
            "decode part ID and serial number",
            "response must contain exactly six little-endian words",
        ));
    }
    let mut serial = [0u32; 4];
    for (word, chunk) in serial.iter_mut().zip(&chunks[2..]) {
        *word = u32::from_le_bytes(*chunk);
    }
    Ok(PartIdSerial { serial })
}

pub(crate) fn decode_c_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_splits_into_little_endian_mhz_and_remainder() {
        let request = VendorControlRequest::set_frequency(2_480_123_456);
        assert_eq!(request.request, VendorRequest::SetFreq);
        assert_eq!(&request.data[..4], &2480_u32.to_le_bytes());
        assert_eq!(&request.data[4..], &123_456_u32.to_le_bytes());
    }

    #[test]
    fn sample_rate_uses_integer_divider_one() {
        let request = VendorControlRequest::set_sample_rate(10_000_000);
        assert_eq!(&request.data[..4], &10_000_000_u32.to_le_bytes());
        assert_eq!(&request.data[4..], &1_u32.to_le_bytes());
    }

    #[test]
    fn full_bandwidth_is_packed_across_value_and_index() {
        let request = VendorControlRequest::set_baseband_bandwidth(20_000_000);
        assert_eq!(request.value, 20_000_000_u32 as u16);
        assert_eq!(request.index, (20_000_000_u32 >> 16) as u16);
    }

    #[test]
    fn init_sweep_splits_the_dwell_and_carries_the_range_list() {
        let payload = vec![0xaa; 13];
        let request = VendorControlRequest::init_sweep(0x0004_0000, payload.clone());
        assert_eq!(request.request, VendorRequest::InitSweep);
        assert_eq!(request.value, 0x0000);
        assert_eq!(request.index, 0x0004);
        assert_eq!(request.data, payload);
        assert_eq!(request.length, 13);
    }

    #[test]
    fn gain_setters_carry_the_value_in_the_index() {
        assert_eq!(VendorControlRequest::set_lna_gain(40).index, 40);
        assert_eq!(VendorControlRequest::set_vga_gain(62).index, 62);
        assert_eq!(VendorControlRequest::set_lna_gain(40).length, 1);
    }

    #[test]
    fn tx_gain_and_flush_size_requests_match_libhackrf() {
        let gain = VendorControlRequest::set_tx_vga_gain(47);
        assert_eq!(gain.request, VendorRequest::SetTxVgaGain);
        assert_eq!(gain.index, 47);
        assert_eq!(gain.length, 1);

        let flush = VendorControlRequest::get_buffer_size();
        assert_eq!(flush.request, VendorRequest::GetBufferSize);
        assert_eq!(flush.length, 4);
    }

    #[test]
    fn a_gain_the_firmware_refuses_is_an_error() {
        assert!(validate_gain_response(&[1], "set LNA gain").is_ok());
        assert!(validate_gain_response(&[0], "set LNA gain").is_err());
        assert!(validate_gain_response(&[], "set LNA gain").is_err());
    }

    #[test]
    fn part_serial_words_join_in_usb_descriptor_order() {
        let words = [
            0x1111_1111_u32,
            0x2222_2222,
            0x0011_2233,
            0x4455_6677,
            0x8899_aabb,
            0xccdd_eeff,
        ];
        let bytes = words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let decoded = decode_part_id_serial(&bytes).expect("six words");
        assert_eq!(
            decoded.serial_u128(),
            Some(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
        );
        assert!(decode_part_id_serial(&bytes[..20]).is_err());
    }

    #[test]
    fn the_version_string_stops_at_the_first_nul() {
        assert_eq!(decode_c_string(b"2024.02.1\0\0\0\0"), "2024.02.1");
        assert_eq!(decode_c_string(b"unterminated"), "unterminated");
    }
}
