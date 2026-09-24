use std::io::Read;

use flate2::read::GzDecoder;

macro_rules! packed_data {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/data/", $name, ".gz"))
    };
}

pub(crate) use packed_data;

pub(crate) fn inflate(gzipped: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    GzDecoder::new(gzipped).read_to_end(&mut raw)?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_data_round_trips_to_the_committed_file() {
        let committed = include_bytes!("../data/bandplan/world.json");
        let unpacked = inflate(packed_data!("bandplan/world.json")).expect("inflate");
        assert_eq!(unpacked, committed);
    }

    #[test]
    fn corrupt_input_is_an_error() {
        assert!(inflate(b"not gzip").is_err());
    }
}
