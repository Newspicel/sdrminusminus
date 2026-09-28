use sdrmm_dsp::crc16_msb;
use sdrmm_wire::BroadcastData;

use self::{
    group::Assembler,
    xpad::{Application, FixedPad, Layout, Size},
};

mod dse;
mod group;
pub(super) mod label;
pub(super) mod mot;
mod xpad;

#[cfg(test)]
mod tests;

const CRC_POLYNOMIAL: u16 = 0x1021;
const CRC_INITIAL: u16 = 0xffff;
const NO_FIXED_PAD: [u8; 2] = [0, 0];

pub enum Event {
    Label(String),
    Object(BroadcastData),
    Error(&'static str),
}

pub fn crc_ok(bytes: &[u8]) -> bool {
    bytes.split_last_chunk::<2>().is_some_and(|(body, check)| {
        !crc16_msb(CRC_POLYNOMIAL, CRC_INITIAL, body) == u16::from_be_bytes(*check)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Continuation {
    application: Application,
    size: Size,
    field_length: usize,
}

#[derive(Default)]
pub struct Pad {
    pub mot_app: Option<u8>,
    pub good: u32,
    pub bad: u32,
    size: Size,
    previous: Option<Continuation>,
    logical: Vec<u8>,
    label: label::Label,
    groups: Assembler,
    mot: mot::Mot,
}

impl Pad {
    pub fn access_unit(&mut self, unit: &[u8], events: &mut Vec<Event>) {
        match dse::pad_field(unit) {
            Ok(Some(field)) => {
                if let Some((xpad, fpad)) = field.split_last_chunk::<2>() {
                    self.process(xpad, *fpad, true, events);
                }
            }
            Ok(None) => self.process(&[], NO_FIXED_PAD, true, events),
            Err(message) => {
                self.previous = None;
                self.fail(message, events);
            }
        }
    }

    pub fn process(&mut self, bytes: &[u8], fpad: [u8; 2], exact: bool, events: &mut Vec<Event>) {
        let fixed = FixedPad::read(fpad);
        if let Some(size) = fixed.size {
            self.size = size;
        }
        let previous = self.previous.take();
        let layout = match self.layout(bytes, fixed.contents_indicated, exact, previous) {
            Ok(layout) => layout,
            Err(message) => return self.fail(message, events),
        };
        let field = &bytes[bytes.len() - layout.field_length..];
        let mut logical = std::mem::take(&mut self.logical);
        logical.clear();
        logical.extend(field.iter().rev());
        let mut last = None;
        for subfield in layout.subfields() {
            let application = match subfield.app_type {
                Some(app_type) => Application::classify(app_type, self.mot_app),
                None => match previous {
                    Some(continuation) => continuation.application.continued(),
                    None => continue,
                },
            };
            self.deliver(application, &logical[subfield.range()], events);
            last = Some(application);
        }
        self.logical = logical;
        self.previous = last.map(|application| Continuation {
            application,
            size: self.size,
            field_length: layout.field_length,
        });
    }

    fn layout(
        &self,
        bytes: &[u8],
        indicated: bool,
        exact: bool,
        previous: Option<Continuation>,
    ) -> Result<Layout, &'static str> {
        if exact && bytes.is_empty() {
            return Ok(Layout::default());
        }
        match (self.size, indicated) {
            (Size::Absent, _) => Ok(Layout::default()),
            (Size::Reserved, _) => Err("Reserved X-PAD indicator"),
            (Size::Short, _) => xpad::short(bytes, indicated, exact),
            (Size::Variable, true) => xpad::variable(bytes, exact),
            (Size::Variable, false) => {
                let field_length = previous
                    .filter(|continuation| continuation.size == Size::Variable)
                    .map(|continuation| continuation.field_length);
                xpad::continued(bytes, field_length, exact)
            }
        }
    }

    fn deliver(&mut self, application: Application, bytes: &[u8], events: &mut Vec<Event>) {
        if !matches!(
            application,
            Application::LengthIndicator { .. } | Application::DataGroup { start: true }
        ) {
            self.groups.interrupt();
        }
        match application {
            Application::LengthIndicator { start } => {
                let result = self.groups.length_indicator(start, bytes);
                self.report(result, events);
            }
            Application::Label { start } => self.label_segment(start, bytes, events),
            Application::DataGroup { start } => self.data_group(start, bytes, events),
            Application::Other(_) => {}
        }
    }

    fn label_segment(&mut self, start: bool, bytes: &[u8], events: &mut Vec<Event>) {
        match self.label.push(start, bytes) {
            Ok(Some(text)) => self.accept(Event::Label(text), events),
            Ok(None) => {}
            Err(message) => self.fail(message, events),
        }
    }

    fn data_group(&mut self, start: bool, bytes: &[u8], events: &mut Vec<Event>) {
        if start {
            if self.groups.abandon() {
                self.fail("Truncated X-PAD MSC data group", events);
            }
            let begun = self.groups.begin();
            self.report(begun, events);
        }
        let Some(group) = self.groups.extend(bytes) else {
            return;
        };
        match self.mot.push(group) {
            Ok(Some(object)) => self.accept(Event::Object(object), events),
            Ok(None) => self.good += 1,
            Err(message) => self.fail(message, events),
        }
    }

    fn report(&mut self, result: Result<(), &'static str>, events: &mut Vec<Event>) {
        if let Err(message) = result {
            self.fail(message, events);
        }
    }

    fn accept(&mut self, event: Event, events: &mut Vec<Event>) {
        self.good += 1;
        events.push(event);
    }

    fn fail(&mut self, message: &'static str, events: &mut Vec<Event>) {
        self.bad += 1;
        events.push(Event::Error(message));
    }
}
