//! HackRF vendor requests and transceiver modes, as libhackrf numbers them.

/// `bRequest` values the HackRF firmware answers on the control endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum VendorRequest {
    SetTransceiverMode = 1,
    SampleRateSet = 6,
    BasebandFilterBandwidthSet = 7,
    BoardIdRead = 14,
    VersionStringRead = 15,
    SetFreq = 16,
    AmpEnable = 17,
    BoardPartIdSerialNoRead = 18,
    SetLnaGain = 19,
    SetVgaGain = 20,
    SetTxVgaGain = 21,
    AntennaEnable = 23,
    InitSweep = 26,
    GetBufferSize = 61,
}

/// The radio is half duplex: exactly one of these is in force at a time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub(crate) enum TransceiverMode {
    Off = 0,
    Receive = 1,
    Transmit = 2,
    /// Receive while the firmware retunes itself between blocks (`hackrf_sweep`'s mode).
    RxSweep = 5,
}
