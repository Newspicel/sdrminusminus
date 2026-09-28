use std::ops::Range;

const MAX_SUBFIELDS: usize = 4;
const SHORT_FIELD_LENGTH: usize = 4;
const SUBFIELD_LENGTHS: [usize; 8] = [4, 6, 8, 12, 16, 24, 32, 48];
const APP_TYPE_MASK: u8 = 0x1f;
const END_MARKER: u8 = 0;
const LENGTH_INDICATOR: u8 = 1;
const LABEL_START: u8 = 2;
const LABEL_CONTINUATION: u8 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Absent,
    Short,
    Variable,
    Reserved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedPad {
    pub size: Option<Size>,
    pub contents_indicated: bool,
}

impl FixedPad {
    pub fn read([info, byte_l]: [u8; 2]) -> Self {
        let size = (info >> 6 == 0).then_some(match (info >> 4) & 3 {
            0 => Size::Absent,
            1 => Size::Short,
            2 => Size::Variable,
            _ => Size::Reserved,
        });
        Self {
            size,
            contents_indicated: byte_l & 2 != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Application {
    LengthIndicator { start: bool },
    Label { start: bool },
    DataGroup { start: bool },
    Other(u8),
}

impl Application {
    pub fn classify(app_type: u8, mot_app: Option<u8>) -> Self {
        match app_type {
            LENGTH_INDICATOR => Self::LengthIndicator { start: true },
            LABEL_START => Self::Label { start: true },
            LABEL_CONTINUATION => Self::Label { start: false },
            _ if mot_app == Some(app_type) => Self::DataGroup { start: true },
            _ if mot_app.and_then(|start| start.checked_add(1)) == Some(app_type) => {
                Self::DataGroup { start: false }
            }
            _ => Self::Other(app_type),
        }
    }

    pub fn continued(self) -> Self {
        match self {
            Self::LengthIndicator { .. } => Self::LengthIndicator { start: false },
            Self::Label { .. } => Self::Label { start: false },
            Self::DataGroup { .. } => Self::DataGroup { start: false },
            Self::Other(app_type) => Self::Other(app_type),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subfield {
    pub app_type: Option<u8>,
    start: usize,
    end: usize,
}

impl Subfield {
    pub fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub field_length: usize,
    subfields: [Option<Subfield>; MAX_SUBFIELDS],
}

impl Layout {
    pub fn subfields(&self) -> impl Iterator<Item = Subfield> + '_ {
        self.subfields.iter().flatten().copied()
    }

    fn single(field_length: usize, app_type: Option<u8>, start: usize) -> Self {
        let mut layout = Self {
            field_length,
            ..Self::default()
        };
        layout.subfields[0] = Some(Subfield {
            app_type,
            start,
            end: field_length,
        });
        layout
    }
}

pub fn short(bytes: &[u8], indicated: bool, exact: bool) -> Result<Layout, &'static str> {
    if exact && bytes.len() != SHORT_FIELD_LENGTH {
        return Err("Short X-PAD length mismatch");
    }
    let field = bytes
        .last_chunk::<SHORT_FIELD_LENGTH>()
        .ok_or("Truncated short X-PAD")?;
    if !indicated {
        return Ok(Layout::single(SHORT_FIELD_LENGTH, None, 0));
    }
    let app_type = field[SHORT_FIELD_LENGTH - 1] & APP_TYPE_MASK;
    if app_type == END_MARKER {
        return Ok(Layout::default());
    }
    Ok(Layout::single(SHORT_FIELD_LENGTH, Some(app_type), 1))
}

pub fn variable(bytes: &[u8], exact: bool) -> Result<Layout, &'static str> {
    let (indicators, list_length) = contents_list(bytes.iter().rev().copied())?;
    if indicators[0].is_none() {
        return Ok(Layout::default());
    }
    let mut layout = Layout {
        field_length: list_length,
        ..Layout::default()
    };
    for (slot, (app_type, length)) in layout
        .subfields
        .iter_mut()
        .zip(indicators.into_iter().flatten())
    {
        let start = layout.field_length;
        layout.field_length += length;
        *slot = Some(Subfield {
            app_type: Some(app_type),
            start,
            end: layout.field_length,
        });
    }
    fits(
        bytes,
        layout.field_length,
        exact,
        "X-PAD length disagrees with contents indicators",
    )?;
    Ok(layout)
}

pub fn continued(
    bytes: &[u8],
    field_length: Option<usize>,
    exact: bool,
) -> Result<Layout, &'static str> {
    let Some(field_length) = field_length else {
        return Ok(Layout::default());
    };
    fits(
        bytes,
        field_length,
        exact,
        "X-PAD length changed without contents indicators",
    )?;
    Ok(Layout::single(field_length, None, 0))
}

fn fits(
    bytes: &[u8],
    field_length: usize,
    exact: bool,
    mismatch: &'static str,
) -> Result<(), &'static str> {
    match (exact, bytes.len()) {
        (true, length) if length != field_length => Err(mismatch),
        (false, length) if length < field_length => Err("Truncated X-PAD"),
        _ => Ok(()),
    }
}

type Indicators = [Option<(u8, usize)>; MAX_SUBFIELDS];

fn contents_list(
    mut logical: impl Iterator<Item = u8>,
) -> Result<(Indicators, usize), &'static str> {
    let mut indicators = Indicators::default();
    let mut list_length = 0;
    for slot in &mut indicators {
        let indicator = logical
            .next()
            .ok_or("Truncated X-PAD contents indicator list")?;
        list_length += 1;
        let app_type = indicator & APP_TYPE_MASK;
        if app_type == END_MARKER {
            break;
        }
        *slot = Some((app_type, SUBFIELD_LENGTHS[usize::from(indicator >> 5)]));
    }
    Ok((indicators, list_length))
}

#[cfg(test)]
mod tests;
