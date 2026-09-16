use super::{Frame, Rate};

mod r100_180_n;
mod r104_180_n;
mod r116_180_n;
mod r11_20_n;
mod r124_180_n;
mod r128_180_n;
mod r132_180_n;
mod r135_180_n;
mod r13_18_n;
mod r13_45_n;
mod r140_180_n;
mod r14_45_s;
mod r154_180_n;
mod r18_30_n;
mod r20_30_n;
mod r22_30_n;
mod r23_36_n;
mod r25_36_n;
mod r26_45_n;
mod r26_45_s;
mod r28_45_n;
mod r32_45_s;
mod r7_15_s;
mod r7_9_n;
mod r8_15_s;
mod r90_180_n;
mod r96_180_n;
mod r9_20_n;

pub fn addresses(rate: Rate, frame: Frame) -> Option<&'static [&'static [u16]]> {
    match (rate, frame) {
        (Rate::R100_180, Frame::Normal) => Some(r100_180_n::ADDRESSES),
        (Rate::R104_180, Frame::Normal) => Some(r104_180_n::ADDRESSES),
        (Rate::R116_180, Frame::Normal) => Some(r116_180_n::ADDRESSES),
        (Rate::R11_20, Frame::Normal) => Some(r11_20_n::ADDRESSES),
        (Rate::R124_180, Frame::Normal) => Some(r124_180_n::ADDRESSES),
        (Rate::R128_180, Frame::Normal) => Some(r128_180_n::ADDRESSES),
        (Rate::R132_180, Frame::Normal) => Some(r132_180_n::ADDRESSES),
        (Rate::R135_180, Frame::Normal) => Some(r135_180_n::ADDRESSES),
        (Rate::R13_18, Frame::Normal) => Some(r13_18_n::ADDRESSES),
        (Rate::R13_45, Frame::Normal) => Some(r13_45_n::ADDRESSES),
        (Rate::R140_180, Frame::Normal) => Some(r140_180_n::ADDRESSES),
        (Rate::R14_45, Frame::Short) => Some(r14_45_s::ADDRESSES),
        (Rate::R154_180, Frame::Normal) => Some(r154_180_n::ADDRESSES),
        (Rate::R18_30, Frame::Normal) => Some(r18_30_n::ADDRESSES),
        (Rate::R20_30, Frame::Normal) => Some(r20_30_n::ADDRESSES),
        (Rate::R22_30, Frame::Normal) => Some(r22_30_n::ADDRESSES),
        (Rate::R23_36, Frame::Normal) => Some(r23_36_n::ADDRESSES),
        (Rate::R25_36, Frame::Normal) => Some(r25_36_n::ADDRESSES),
        (Rate::R26_45, Frame::Normal) => Some(r26_45_n::ADDRESSES),
        (Rate::R26_45, Frame::Short) => Some(r26_45_s::ADDRESSES),
        (Rate::R28_45, Frame::Normal) => Some(r28_45_n::ADDRESSES),
        (Rate::R32_45, Frame::Short) => Some(r32_45_s::ADDRESSES),
        (Rate::R7_15, Frame::Short) => Some(r7_15_s::ADDRESSES),
        (Rate::R7_9, Frame::Normal) => Some(r7_9_n::ADDRESSES),
        (Rate::R8_15, Frame::Short) => Some(r8_15_s::ADDRESSES),
        (Rate::R90_180, Frame::Normal) => Some(r90_180_n::ADDRESSES),
        (Rate::R96_180, Frame::Normal) => Some(r96_180_n::ADDRESSES),
        (Rate::R9_20, Frame::Normal) => Some(r9_20_n::ADDRESSES),
        _ => None,
    }
}
