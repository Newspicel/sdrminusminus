const OCTET_STREAM: &str = "application/octet-stream";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Category {
    #[default]
    GeneralData,
    Text,
    Image,
    Audio,
    Video,
    MotTransport,
    System,
    Application,
    Proprietary,
    Reserved,
}

impl Category {
    fn from_code(code: usize) -> Self {
        match code {
            0b00_0000 => Self::GeneralData,
            0b00_0001 => Self::Text,
            0b00_0010 => Self::Image,
            0b00_0011 => Self::Audio,
            0b00_0100 => Self::Video,
            0b00_0101 => Self::MotTransport,
            0b00_0110 => Self::System,
            0b00_0111 => Self::Application,
            0b11_1111 => Self::Proprietary,
            _ => Self::Reserved,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentType {
    pub category: Category,
    pub subtype: usize,
}

impl ContentType {
    pub fn new(code: usize, subtype: usize) -> Self {
        Self {
            category: Category::from_code(code),
            subtype,
        }
    }

    pub fn media_type(self) -> &'static str {
        match (self.category, self.subtype) {
            (Category::Text, 0) => "text/plain",
            (Category::Text, 1) => "text/plain; charset=iso-8859-1",
            (Category::Text, 2) => "text/html",
            (Category::Text, 3) => "application/pdf",
            (Category::Image, 1) => "image/jpeg",
            (Category::Image, 3) => "image/png",
            (Category::Audio, 1 | 4) => "audio/mpeg",
            (Category::Audio, 10) => "audio/mp4",
            (Category::Video, 2) => "video/mp4",
            _ => OCTET_STREAM,
        }
    }
}
