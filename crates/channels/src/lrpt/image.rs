use super::{
    jpeg::{Block, MCU_SIDE, MCUS_PER_PACKET, McuDecoder},
    link::{PACKET_HEADER, SEQUENCE_MODULO},
};
use crate::VideoPicture;

pub const MCUS_PER_ROW: usize = 196;
pub const WIDTH: usize = MCUS_PER_ROW * MCU_SIDE;
pub const MAX_LINES: usize = 6_144;
pub const MAX_ROWS: usize = MAX_LINES / MCU_SIDE;
pub const FIRST_APID: u16 = 64;
pub const LAST_APID: u16 = 69;
pub const TELEMETRY_APID: u16 = 70;
pub const IDLE_APID: u16 = 0x7FF;
pub const BLUE_APID: u16 = 64;
pub const RED_APID: u16 = 65;
pub const PACKETS_PER_ROW: i64 = 3 * MCUS_PER_PACKET as i64 + 1;
pub const TIME_BYTES: usize = 8;
pub const MCU_ID_AT: usize = TIME_BYTES;
pub const QUALITY_AT: usize = TIME_BYTES + 5;
pub const IMAGE_HEADER: usize = TIME_BYTES + 6;
const SLOTS: usize = 3;
const RANK_SPAN: i64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Placed,
    Ignored,
    Damaged,
    Full,
}

struct Plane {
    apid: Option<u16>,
    pixels: Vec<u8>,
    blocks: u32,
}

impl Plane {
    fn new() -> Self {
        Self {
            apid: None,
            pixels: vec![0; WIDTH * MAX_LINES],
            blocks: 0,
        }
    }

    fn clear(&mut self, rows: usize) {
        self.pixels[..rows * MCU_SIDE * WIDTH].fill(0);
        self.apid = None;
        self.blocks = 0;
    }

    fn put(&mut self, row: usize, column: usize, block: &Block) {
        for (y, line) in block.as_chunks::<MCU_SIDE>().0.iter().enumerate() {
            let start = (row * MCU_SIDE + y) * WIDTH + column * MCU_SIDE;
            self.pixels[start..start + MCU_SIDE].copy_from_slice(line);
        }
        self.blocks += 1;
    }
}

pub struct Imagery {
    planes: [Plane; SLOTS],
    anchor: Option<i64>,
    sequence: Option<(u16, i64)>,
    rows: usize,
    packets_lost: u32,
    block: Block,
}

fn row_of(distance: i64) -> Option<usize> {
    (-RANK_SPAN..=RANK_SPAN)
        .map(|rank| distance - rank * MCUS_PER_PACKET as i64)
        .find(|offset| offset.rem_euclid(PACKETS_PER_ROW) == 0)
        .and_then(|offset| usize::try_from(offset / PACKETS_PER_ROW).ok())
}

impl Imagery {
    #[must_use]
    pub fn new() -> Self {
        Self {
            planes: std::array::from_fn(|_| Plane::new()),
            anchor: None,
            sequence: None,
            rows: 0,
            packets_lost: 0,
            block: [0; MCU_SIDE * MCU_SIDE],
        }
    }

    pub fn reset(&mut self) {
        let rows = self.rows;
        for plane in &mut self.planes {
            plane.clear(rows);
        }
        self.anchor = None;
        self.sequence = None;
        self.rows = 0;
        self.packets_lost = 0;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    #[must_use]
    pub fn lines(&self) -> u16 {
        (self.rows * MCU_SIDE) as u16
    }

    #[must_use]
    pub fn packets_lost(&self) -> u32 {
        self.packets_lost
    }

    pub fn count_lost(&mut self, packets: u32) {
        self.packets_lost = self.packets_lost.saturating_add(packets);
    }

    #[must_use]
    pub fn apids(&self) -> Vec<u16> {
        let mut apids: Vec<u16> = self.planes.iter().filter_map(|plane| plane.apid).collect();
        apids.sort_unstable();
        apids
    }

    fn unwrap_sequence(&mut self, raw: u16) -> i64 {
        let modulo = i64::from(SEQUENCE_MODULO);
        let unwrapped = match self.sequence {
            None => i64::from(raw),
            Some((previous_raw, previous)) => {
                let mut delta = (i64::from(raw) - i64::from(previous_raw)).rem_euclid(modulo);
                if delta >= modulo / 2 {
                    delta -= modulo;
                }
                if delta > 1 {
                    self.count_lost((delta - 1) as u32);
                }
                previous + delta
            }
        };
        if self
            .sequence
            .is_none_or(|(_, previous)| unwrapped > previous)
        {
            self.sequence = Some((raw, unwrapped));
        }
        unwrapped
    }

    fn slot(&mut self, apid: u16) -> Option<usize> {
        self.planes
            .iter()
            .position(|plane| plane.apid == Some(apid))
            .or_else(|| {
                let free = self.planes.iter().position(|plane| plane.apid.is_none())?;
                self.planes[free].apid = Some(apid);
                Some(free)
            })
    }

    pub fn place(&mut self, packet: &[u8], apid: u16, raw_sequence: u16) -> Placement {
        let sequence = self.unwrap_sequence(raw_sequence);
        if !(FIRST_APID..=LAST_APID).contains(&apid) {
            return Placement::Ignored;
        }
        let data = &packet[PACKET_HEADER.min(packet.len())..];
        if data.len() < IMAGE_HEADER {
            self.count_lost(1);
            return Placement::Damaged;
        }
        let first_mcu = usize::from(data[MCU_ID_AT]);
        if first_mcu >= MCUS_PER_ROW {
            self.count_lost(1);
            return Placement::Damaged;
        }
        let position = sequence - (first_mcu / MCUS_PER_PACKET) as i64;
        let anchor = *self.anchor.get_or_insert(position);
        let Some(row) = row_of(position - anchor) else {
            self.count_lost(1);
            return Placement::Damaged;
        };
        if row >= MAX_ROWS {
            return Placement::Full;
        }
        let Some(slot) = self.slot(apid) else {
            return Placement::Ignored;
        };
        self.rows = self.rows.max(row + 1);
        self.decode(slot, row, first_mcu, data)
    }

    fn decode(&mut self, slot: usize, row: usize, first_mcu: usize, data: &[u8]) -> Placement {
        let mut decoder = McuDecoder::new(&data[IMAGE_HEADER..], data[QUALITY_AT]);
        for column in first_mcu..(first_mcu + MCUS_PER_PACKET).min(MCUS_PER_ROW) {
            if decoder.next_block(&mut self.block).is_none() {
                self.count_lost(1);
                return Placement::Damaged;
            }
            self.planes[slot].put(row, column, &self.block);
        }
        Placement::Placed
    }

    fn plane(&self, apid: u16) -> Option<&Plane> {
        self.planes.iter().find(|plane| plane.apid == Some(apid))
    }

    #[must_use]
    pub fn label(&self) -> String {
        match self.composite() {
            Some(_) => "LRPT 221".to_owned(),
            None => self
                .strongest()
                .and_then(|plane| plane.apid)
                .map_or_else(|| "LRPT".to_owned(), |apid| format!("LRPT APID {apid}")),
        }
    }

    fn composite(&self) -> Option<(&Plane, &Plane)> {
        Some((self.plane(RED_APID)?, self.plane(BLUE_APID)?))
    }

    fn strongest(&self) -> Option<&Plane> {
        self.planes
            .iter()
            .filter(|plane| plane.apid.is_some())
            .max_by_key(|plane| plane.blocks)
    }

    #[must_use]
    pub fn snapshot(&self) -> VideoPicture {
        let pixels = self.rows * MCU_SIDE * WIDTH;
        let mut rgb = vec![0u8; pixels * 3];
        let mut luma = vec![0u8; pixels];
        if let Some((red, blue)) = self.composite() {
            for (index, (pixel, value)) in rgb
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(&mut luma)
                .enumerate()
            {
                let (r, b) = (red.pixels[index], blue.pixels[index]);
                *pixel = [r, r, b];
                *value = ((u32::from(r) * 886 + u32::from(b) * 114) / 1000) as u8;
            }
        } else if let Some(plane) = self.strongest() {
            luma.copy_from_slice(&plane.pixels[..pixels]);
            for (pixel, &value) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(&luma) {
                *pixel = [value; 3];
            }
        }
        VideoPicture {
            width: WIDTH as u16,
            height: self.lines(),
            luma,
            rgb,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_the_shared_counter_whatever_channel_comes_first() {
        assert_eq!(row_of(0), Some(0));
        assert_eq!(row_of(14), Some(0));
        assert_eq!(row_of(-28), Some(0));
        assert_eq!(row_of(43 - 28), Some(1));
        assert_eq!(row_of(43 * 7 + 28), Some(7));
        assert_eq!(row_of(-43), None);
        assert_eq!(row_of(5), None);
    }

    #[test]
    fn a_sequence_gap_counts_lost_packets() {
        let mut imagery = Imagery::new();
        imagery.unwrap_sequence(16_380);
        imagery.unwrap_sequence(16_381);
        assert_eq!(imagery.unwrap_sequence(3), 16_387);
        assert_eq!(imagery.packets_lost(), 5);
    }
}
