use sdrmm_wire::DataLinkMessage;
use xng_types::{AppInfo, Message, Provenance, StationIdentity};

use crate::datalink::{self, Quality};

pub fn provenance() -> Provenance {
    Provenance {
        station: StationIdentity::new("SDR--"),
        app: AppInfo {
            name: "SDR--".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        sdr: None,
        channel: None,
    }
}

pub fn structured(message: Message) -> DataLinkMessage {
    datalink::message(
        &message.body,
        Quality {
            crc_ok: message.decode.crc_ok,
            fec_corrected: message.decode.fec_corrected,
            snr_db: message.signal.snr_db,
            frequency_error_hz: message.signal.freq_skew_hz,
        },
        message.raw.as_deref(),
    )
}
