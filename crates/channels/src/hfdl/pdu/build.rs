use sdrmm_dsp::crc16_x25;

pub fn with_fcs(mut frame: Vec<u8>) -> Vec<u8> {
    let fcs = crc16_x25(&frame);
    frame.extend(fcs.to_le_bytes());
    frame
}

pub fn spdu(gs_id: u8, frame_index: u16, systable_version: u16) -> Vec<u8> {
    let mut p = vec![0u8; 64];
    p[0] = 0b0000_0100;
    p[1] = gs_id & 0x7F | 0x80;
    p[2] = (frame_index & 0xFF) as u8;
    p[3] = ((frame_index >> 8) & 0x0F) as u8;
    p[53] = (systable_version & 0xFF) as u8;
    p[54] = ((systable_version >> 8) & 0x0F) as u8 | 0x10;
    with_fcs(p)
}

pub fn lpdu_hfnpdu(hfnpdu: &[u8]) -> Vec<u8> {
    let mut lpdu = vec![0x0D];
    lpdu.extend_from_slice(hfnpdu);
    with_fcs(lpdu)
}

pub fn lpdu_acars(acars_block: &[u8]) -> Vec<u8> {
    let mut lpdu = vec![0x0D, 0xFF, 0xFF];
    lpdu.extend_from_slice(acars_block);
    with_fcs(lpdu)
}

fn wrap(mut header: Vec<u8>, lpdus: &[Vec<u8>]) -> Vec<u8> {
    header.extend(lpdus.iter().map(|l| (l.len() - 1) as u8));
    let mut p = with_fcs(header);
    for lpdu in lpdus {
        p.extend_from_slice(lpdu);
    }
    p
}

pub fn mpdu_downlink(gs_id: u8, aircraft_id: u8, lpdus: &[Vec<u8>]) -> Vec<u8> {
    let header = vec![
        0b0000_0011 | ((lpdus.len() as u8 & 0x0F) << 2),
        gs_id & 0x7F,
        aircraft_id,
        0,
        0,
        0,
    ];
    wrap(header, lpdus)
}

pub fn mpdu_uplink(gs_id: u8, aircraft_id: u8, lpdus: &[Vec<u8>]) -> Vec<u8> {
    let header = vec![
        0b0000_0001,
        gs_id & 0x7F,
        aircraft_id,
        (lpdus.len() as u8) << 4,
    ];
    wrap(header, lpdus)
}

pub fn lpdu_logon_confirm(icao: u32, assigned_id: u8) -> Vec<u8> {
    let b = icao.to_be_bytes();
    with_fcs(vec![
        0x9F,
        b[1].reverse_bits(),
        b[2].reverse_bits(),
        b[3].reverse_bits(),
        assigned_id,
    ])
}

pub fn acars_block() -> Vec<u8> {
    crate::acars::block::build(
        '2',
        "N471XG",
        None,
        "B6",
        '4',
        Some("M11A"),
        Some("UA0042"),
        "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
        false,
    )
}

pub fn acars_mpdu() -> Vec<u8> {
    mpdu_downlink(3, 0xC7, &[lpdu_acars(&acars_block())])
}
