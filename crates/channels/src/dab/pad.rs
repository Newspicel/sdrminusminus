use sdrmm_dsp::crc16_msb;
use sdrmm_wire::BroadcastData;

pub(super) mod label;
pub(super) mod mot;

#[cfg(test)]
mod tests;

pub enum Event {
    Label(String),
    Object(BroadcastData),
    Error(&'static str),
}

pub fn crc_ok(bytes: &[u8]) -> bool {
    bytes.len() >= 2
        && !crc16_msb(0x1021, 0xffff, &bytes[..bytes.len() - 2])
            == u16::from_be_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]])
}

#[derive(Default)]
pub struct Pad {
    previous: Option<(u8, usize)>,
    label: label::Label,
    length: Vec<u8>,
    pending_length: usize,
    group: Vec<u8>,
    group_length: usize,
    mot: mot::Mot,
    pub mot_app: Option<u8>,
    pub good: u32,
    pub bad: u32,
}

impl Pad {
    pub fn access_unit(&mut self, unit: &[u8], events: &mut Vec<Event>) {
        if unit.first().is_none_or(|byte| byte >> 5 != 4) {
            self.previous = None;
            return;
        }
        let Some(&count) = unit.get(1) else { return };
        let (offset, length) = if count == 255 {
            let Some(&extra) = unit.get(2) else { return };
            (3, 255 + usize::from(extra))
        } else {
            (2, usize::from(count))
        };
        let Some(data) = unit
            .get(offset..offset + length)
            .filter(|data| data.len() >= 2)
        else {
            self.fail("Truncated DAB+ PAD element", events);
            return;
        };
        self.process(
            &data[..length - 2],
            [data[length - 2], data[length - 1]],
            true,
            events,
        );
    }

    fn fail(&mut self, message: &'static str, events: &mut Vec<Event>) {
        self.bad = self.bad.saturating_add(1);
        events.push(Event::Error(message));
    }

    pub fn process(&mut self, bytes: &[u8], fpad: [u8; 2], exact: bool, events: &mut Vec<Event>) {
        let previous = self.previous.take();
        if fpad[0] >> 6 != 0 || !matches!((fpad[0] >> 4) & 3, 1 | 2) {
            return;
        }
        let ci = fpad[1] & 2 != 0;
        let mut reversed = [0u8; 196];
        let length = bytes.len().min(reversed.len());
        for (target, source) in reversed.iter_mut().zip(bytes.iter().rev()).take(length) {
            *target = *source;
        }
        let data = &reversed[..length];
        let mut fields = [(0u8, 0usize); 4];
        let mut count = 0;
        let mut offset = 0;
        if ci {
            if fpad[0] & 0x30 == 0x10 {
                if let Some(&first) = data.first().filter(|&&v| v & 31 != 0) {
                    fields[0] = (first & 31, 3);
                    count = 1;
                    offset = 1;
                }
            } else {
                const LENGTHS: [usize; 8] = [4, 6, 8, 12, 16, 24, 32, 48];
                for &header in data.iter().take(4) {
                    offset += 1;
                    if header & 31 == 0 {
                        break;
                    }
                    fields[count] = (header & 31, LENGTHS[usize::from(header >> 5)]);
                    count += 1;
                }
            }
        } else if let Some(previous) = previous {
            fields[0] = previous;
            count = 1;
        }
        if count == 0 {
            return;
        }
        let announced = offset + fields[..count].iter().map(|(_, len)| len).sum::<usize>();
        if announced > data.len() || (exact && announced != bytes.len()) {
            self.fail("DAB PAD length mismatch", events);
            return;
        }
        for &(kind, size) in &fields[..count] {
            let part = &data[offset..offset + size];
            let group_length = std::mem::take(&mut self.pending_length);
            let continuation = match kind {
                1 => {
                    if ci {
                        self.length.clear();
                    }
                    self.length
                        .extend(part.iter().take(4usize.saturating_sub(self.length.len())));
                    if self.length.len() == 4 {
                        if crc_ok(&self.length) {
                            self.pending_length = (usize::from(self.length[0] & 63) << 8)
                                | usize::from(self.length[1]);
                        } else {
                            self.fail("DAB data-group length CRC failure", events);
                        }
                        self.length.clear();
                    }
                    Some(1)
                }
                2 | 3 => {
                    match self.label.push(kind == 2, part) {
                        Ok(Some(text)) => {
                            self.good = self.good.saturating_add(1);
                            events.push(Event::Label(text));
                        }
                        Ok(None) => {}
                        Err(error) => self.fail(error, events),
                    }
                    Some(3)
                }
                app if self
                    .mot_app
                    .is_some_and(|start| app == start || app == start + 1) =>
                {
                    if self.mot_app == Some(app) {
                        self.group.clear();
                        self.group_length = group_length;
                    }
                    if (11..=16383).contains(&self.group_length)
                        && self.group.len() < self.group_length
                    {
                        self.group
                            .extend(part.iter().take(self.group_length - self.group.len()));
                        if self.group.len() == self.group_length {
                            match self.mot.push(&self.group) {
                                Ok(object) => {
                                    self.good = self.good.saturating_add(1);
                                    if let Some(object) = object {
                                        events.push(Event::Object(object));
                                    }
                                }
                                Err(error) => self.fail(error, events),
                            }
                        }
                    }
                    self.mot_app.map(|app| app + 1)
                }
                _ => None,
            };
            offset += size;
            self.previous = continuation.map(|kind| (kind, announced));
        }
    }
}
