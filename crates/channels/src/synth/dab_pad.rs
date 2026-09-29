use sdrmm_dsp::crc16_msb;

fn crc(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.extend_from_slice(&(!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
    bytes
}

pub(super) fn group(kind: u8, index: usize, last: bool, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![
        0x70 | kind,
        0,
        if last { 128 } else { 0 },
        index as u8,
        0x12,
        0x12,
        0x34,
    ];
    bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
    bytes.extend_from_slice(data);
    crc(bytes)
}

pub(super) fn header(length: usize) -> Vec<u8> {
    let mut bytes = vec![
        0,
        0,
        (length >> 4) as u8,
        (length << 4) as u8,
        9,
        0x84,
        3,
        0xcc,
        10,
        0xf0,
    ];
    bytes.extend_from_slice(b"slide.png");
    bytes
}

pub fn prepend(unit: &[u8], index: usize) -> Vec<u8> {
    let image = include_bytes!("../../../../fixtures/broadcast_audio/slideshow.png");
    let cycle = index % (2 + image.len().div_ceil(32));
    let mut pad = if cycle == 0 {
        let mut label = vec![0x69, 0xf0];
        label.extend_from_slice(b"SDR-- live");
        let mut pad = vec![0x82, 0];
        pad.extend_from_slice(&crc(label));
        pad.resize(18, 0);
        pad
    } else {
        let group = if cycle == 1 {
            group(3, 0, true, &header(image.len()))
        } else {
            let at = (cycle - 2) * 32;
            let end = (at + 32).min(image.len());
            group(4, cycle - 2, end == image.len(), &image[at..end])
        };
        let indicator = crc((group.len() as u16).to_be_bytes().to_vec());
        let mut pad = vec![1, 0xec, 0];
        pad.extend_from_slice(&indicator);
        pad.extend_from_slice(&group);
        pad.resize(55, 0);
        pad
    };
    pad.reverse();
    pad.extend_from_slice(&[0x20, 2]);
    let mut result = vec![0x80, pad.len() as u8];
    result.extend_from_slice(&pad);
    result.extend_from_slice(unit);
    result
}
