use super::{Mode, Modulation, Rate};

mod m132;
mod m134;
mod m136;
mod m138;
mod m140;
mod m142;
mod m144;
mod m146;
mod m148;
mod m150;
mod m152;
mod m154;
mod m156;
mod m158;
mod m160;
mod m162;
mod m164;
mod m166;
mod m168;
mod m170;
mod m172;
mod m174;
mod m178;
mod m180;
mod m182;
mod m184;
mod m186;
mod m190;
mod m194;
mod m198;
mod m200;
mod m202;
mod m204;
mod m206;
mod m208;
mod m210;
mod m212;
mod m214;
mod m216;
mod m218;
mod m220;
mod m222;
mod m224;
mod m226;
mod m228;
mod m230;
mod m232;
mod m234;
mod m236;
mod m238;
mod m240;
mod m242;
mod m244;
mod m246;
mod m248;

pub const MODES: &[Mode] = &[
    Mode {
        code: 132,
        short: false,
        modulation: Modulation::Qpsk,
        rate: Rate::R13_45,
        order: &[0, 0],
        points: m132::POINTS,
    },
    Mode {
        code: 134,
        short: false,
        modulation: Modulation::Qpsk,
        rate: Rate::R9_20,
        order: &[0, 0],
        points: m134::POINTS,
    },
    Mode {
        code: 136,
        short: false,
        modulation: Modulation::Qpsk,
        rate: Rate::R11_20,
        order: &[0, 0],
        points: m136::POINTS,
    },
    Mode {
        code: 138,
        short: false,
        modulation: Modulation::Apsk8,
        rate: Rate::R100_180,
        order: &[0, 1, 2],
        points: m138::POINTS,
    },
    Mode {
        code: 140,
        short: false,
        modulation: Modulation::Apsk8,
        rate: Rate::R104_180,
        order: &[0, 1, 2],
        points: m140::POINTS,
    },
    Mode {
        code: 142,
        short: false,
        modulation: Modulation::Psk8,
        rate: Rate::R23_36,
        order: &[0, 1, 2],
        points: m142::POINTS,
    },
    Mode {
        code: 144,
        short: false,
        modulation: Modulation::Psk8,
        rate: Rate::R25_36,
        order: &[1, 0, 2],
        points: m144::POINTS,
    },
    Mode {
        code: 146,
        short: false,
        modulation: Modulation::Psk8,
        rate: Rate::R13_18,
        order: &[1, 0, 2],
        points: m146::POINTS,
    },
    Mode {
        code: 148,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R90_180,
        order: &[3, 2, 1, 0],
        points: m148::POINTS,
    },
    Mode {
        code: 150,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R96_180,
        order: &[2, 3, 1, 0],
        points: m150::POINTS,
    },
    Mode {
        code: 152,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R100_180,
        order: &[2, 3, 0, 1],
        points: m152::POINTS,
    },
    Mode {
        code: 154,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R26_45,
        order: &[3, 2, 0, 1],
        points: m154::POINTS,
    },
    Mode {
        code: 156,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R3_5,
        order: &[3, 2, 1, 0],
        points: m156::POINTS,
    },
    Mode {
        code: 158,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R18_30,
        order: &[0, 1, 2, 3],
        points: m158::POINTS,
    },
    Mode {
        code: 160,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R28_45,
        order: &[3, 0, 1, 2],
        points: m160::POINTS,
    },
    Mode {
        code: 162,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R23_36,
        order: &[3, 0, 2, 1],
        points: m162::POINTS,
    },
    Mode {
        code: 164,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R20_30,
        order: &[0, 1, 2, 3],
        points: m164::POINTS,
    },
    Mode {
        code: 166,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R25_36,
        order: &[2, 3, 1, 0],
        points: m166::POINTS,
    },
    Mode {
        code: 168,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R13_18,
        order: &[3, 0, 2, 1],
        points: m168::POINTS,
    },
    Mode {
        code: 170,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R140_180,
        order: &[3, 2, 1, 0],
        points: m170::POINTS,
    },
    Mode {
        code: 172,
        short: false,
        modulation: Modulation::Apsk16,
        rate: Rate::R154_180,
        order: &[0, 3, 2, 1],
        points: m172::POINTS,
    },
    Mode {
        code: 174,
        short: false,
        modulation: Modulation::Apsk32,
        rate: Rate::R2_3,
        order: &[2, 1, 4, 3, 0],
        points: m174::POINTS,
    },
    Mode {
        code: 178,
        short: false,
        modulation: Modulation::Apsk32,
        rate: Rate::R128_180,
        order: &[4, 0, 3, 1, 2],
        points: m178::POINTS,
    },
    Mode {
        code: 180,
        short: false,
        modulation: Modulation::Apsk32,
        rate: Rate::R132_180,
        order: &[4, 0, 3, 1, 2],
        points: m180::POINTS,
    },
    Mode {
        code: 182,
        short: false,
        modulation: Modulation::Apsk32,
        rate: Rate::R140_180,
        order: &[4, 0, 2, 1, 3],
        points: m182::POINTS,
    },
    Mode {
        code: 184,
        short: false,
        modulation: Modulation::Apsk64,
        rate: Rate::R128_180,
        order: &[3, 0, 5, 2, 1, 4],
        points: m184::POINTS,
    },
    Mode {
        code: 186,
        short: false,
        modulation: Modulation::Apsk64,
        rate: Rate::R132_180,
        order: &[5, 2, 0, 1, 4, 3],
        points: m186::POINTS,
    },
    Mode {
        code: 190,
        short: false,
        modulation: Modulation::Apsk64,
        rate: Rate::R7_9,
        order: &[2, 0, 1, 5, 4, 3],
        points: m190::POINTS,
    },
    Mode {
        code: 194,
        short: false,
        modulation: Modulation::Apsk64,
        rate: Rate::R4_5,
        order: &[1, 2, 4, 0, 5, 3],
        points: m194::POINTS,
    },
    Mode {
        code: 198,
        short: false,
        modulation: Modulation::Apsk64,
        rate: Rate::R5_6,
        order: &[4, 2, 1, 0, 5, 3],
        points: m198::POINTS,
    },
    Mode {
        code: 200,
        short: false,
        modulation: Modulation::Apsk128,
        rate: Rate::R135_180,
        order: &[4, 2, 5, 0, 3, 1, 6],
        points: m200::POINTS,
    },
    Mode {
        code: 202,
        short: false,
        modulation: Modulation::Apsk128,
        rate: Rate::R140_180,
        order: &[4, 1, 3, 0, 2, 5, 6],
        points: m202::POINTS,
    },
    Mode {
        code: 204,
        short: false,
        modulation: Modulation::Apsk256,
        rate: Rate::R116_180,
        order: &[4, 0, 3, 7, 2, 1, 5, 6],
        points: m204::POINTS,
    },
    Mode {
        code: 206,
        short: false,
        modulation: Modulation::Apsk256,
        rate: Rate::R20_30,
        order: &[0, 1, 2, 3, 4, 5, 6, 7],
        points: m206::POINTS,
    },
    Mode {
        code: 208,
        short: false,
        modulation: Modulation::Apsk256,
        rate: Rate::R124_180,
        order: &[4, 6, 3, 2, 0, 5, 7, 1],
        points: m208::POINTS,
    },
    Mode {
        code: 210,
        short: false,
        modulation: Modulation::Apsk256,
        rate: Rate::R128_180,
        order: &[7, 5, 6, 4, 2, 3, 0, 1],
        points: m210::POINTS,
    },
    Mode {
        code: 212,
        short: false,
        modulation: Modulation::Apsk256,
        rate: Rate::R22_30,
        order: &[0, 1, 2, 3, 4, 5, 6, 7],
        points: m212::POINTS,
    },
    Mode {
        code: 214,
        short: false,
        modulation: Modulation::Apsk256,
        rate: Rate::R135_180,
        order: &[5, 0, 7, 4, 3, 6, 1, 2],
        points: m214::POINTS,
    },
    Mode {
        code: 216,
        short: true,
        modulation: Modulation::Qpsk,
        rate: Rate::R11_45,
        order: &[0, 0],
        points: m216::POINTS,
    },
    Mode {
        code: 218,
        short: true,
        modulation: Modulation::Qpsk,
        rate: Rate::R4_15,
        order: &[0, 0],
        points: m218::POINTS,
    },
    Mode {
        code: 220,
        short: true,
        modulation: Modulation::Qpsk,
        rate: Rate::R14_45,
        order: &[0, 0],
        points: m220::POINTS,
    },
    Mode {
        code: 222,
        short: true,
        modulation: Modulation::Qpsk,
        rate: Rate::R7_15,
        order: &[0, 0],
        points: m222::POINTS,
    },
    Mode {
        code: 224,
        short: true,
        modulation: Modulation::Qpsk,
        rate: Rate::R8_15,
        order: &[0, 0],
        points: m224::POINTS,
    },
    Mode {
        code: 226,
        short: true,
        modulation: Modulation::Qpsk,
        rate: Rate::R32_45,
        order: &[0, 0],
        points: m226::POINTS,
    },
    Mode {
        code: 228,
        short: true,
        modulation: Modulation::Psk8,
        rate: Rate::R7_15,
        order: &[1, 0, 2],
        points: m228::POINTS,
    },
    Mode {
        code: 230,
        short: true,
        modulation: Modulation::Psk8,
        rate: Rate::R8_15,
        order: &[1, 0, 2],
        points: m230::POINTS,
    },
    Mode {
        code: 232,
        short: true,
        modulation: Modulation::Psk8,
        rate: Rate::R26_45,
        order: &[1, 0, 2],
        points: m232::POINTS,
    },
    Mode {
        code: 234,
        short: true,
        modulation: Modulation::Psk8,
        rate: Rate::R32_45,
        order: &[0, 1, 2],
        points: m234::POINTS,
    },
    Mode {
        code: 236,
        short: true,
        modulation: Modulation::Apsk16,
        rate: Rate::R7_15,
        order: &[2, 1, 0, 3],
        points: m236::POINTS,
    },
    Mode {
        code: 238,
        short: true,
        modulation: Modulation::Apsk16,
        rate: Rate::R8_15,
        order: &[2, 1, 0, 3],
        points: m238::POINTS,
    },
    Mode {
        code: 240,
        short: true,
        modulation: Modulation::Apsk16,
        rate: Rate::R26_45,
        order: &[2, 1, 3, 0],
        points: m240::POINTS,
    },
    Mode {
        code: 242,
        short: true,
        modulation: Modulation::Apsk16,
        rate: Rate::R3_5,
        order: &[3, 2, 0, 1],
        points: m242::POINTS,
    },
    Mode {
        code: 244,
        short: true,
        modulation: Modulation::Apsk16,
        rate: Rate::R32_45,
        order: &[0, 1, 2, 3],
        points: m244::POINTS,
    },
    Mode {
        code: 246,
        short: true,
        modulation: Modulation::Apsk32,
        rate: Rate::R2_3,
        order: &[4, 1, 2, 3, 0],
        points: m246::POINTS,
    },
    Mode {
        code: 248,
        short: true,
        modulation: Modulation::Apsk32,
        rate: Rate::R32_45,
        order: &[1, 0, 4, 2, 3],
        points: m248::POINTS,
    },
];
