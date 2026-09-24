use serde_json::Value;

use super::{
    AcarsCore, block, codec, decode, miam, min, ohma,
    reasm::{Reasm, Reassembler},
    summary,
};

const SAMPLES: &[(&str, &str)] = &[
    (
        "B6",
        "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
    ),
    (
        "B6",
        "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F4",
    ),
    (
        "B6",
        "/AUHASMO.ADS.A6-PFE0724D9586A36C92B2DCF1F0E74A8E4807C0F7219AF407C10422E9E08A1C4",
    ),
    (
        "B6",
        "/CTUE1YA.ADS.HB-JNB1424AB686D9308CA2EBA1D0D24A2C06C1B48CA004A248050667908CA004BF6",
    ),
    (
        "B6",
        "/YQXE2YA.ADS.SP-LRH1424FD087806C0B527769F0D2500B877ED00B5401E2516707755C01340B768",
    ),
    (
        "AA",
        "/AKLCDYA.AT1.9M-MTB215B659D84995674293583561CB9906744E9AF40F9EB",
    ),
    (
        "A6",
        "/AKLCDYA.AT1.9M-MTB215B659D84995674293583561CB9906744E9AF40F9EB",
    ),
    ("BA", "/MSTEC7X.AT1.N123AB0280004A2C"),
    ("B6", "/MSTEC7X.DIS.N123AB8000FFFF"),
    ("B6", "/MSTEC7X.CR1.N123AB0102030405"),
    ("B6", "/SHORTX.ADS.A"),
    ("H1", "#DFB/M1 ENGINE DATA"),
    ("H1", "#M1BPOSRPT"),
    ("H1", "- #MDTEXT"),
    ("H1", "#DFB00000/V206,05,124,183,02,00,00000/V3XX"),
    ("H1", "#CFBFLR/FR19121418400034433406TCAS (1SG)"),
    ("H1", "#CFBAPM_REPORT_A_20200805180631S.CSV"),
    ("H1", "#CFBWRN/WN19121418390034000006NAV TCAS FAULT"),
    ("H1", "#CFBMPF/               /AN.N660AW/FIAAL652"),
    ("H1", "#CFB.1/FLR/FR1602082254 27513406ADR1 X2,ADR3X,ADR2X"),
    ("H1", "#CFB.01/WRN"),
    (
        "H1",
        "#CFBATA\r\n\r\nVIA-1//20DEC//1653//1026//0//DU-6 LOW BRIGHTNESS..",
    ),
    ("H1", "#CFBAL AIR TEMP            32.5 C"),
    ("H1", "#CFBFDE1807300805ABD"),
    ("H1", "#CFBECT FAULT     CH-A"),
    ("H1", "#CFBLIGHTS\r\n R PRIM NAV LT      DS23"),
    ("H1", "#CFBMIL\r\nR N1 VIBES                 0.2 MIL"),
    ("H1", "#CFBPAGE 00001\r\nMDC REPORT: ENGINE TREND"),
    ("H1", "#CFB/1315//38//0//RA-1"),
    (
        "H1",
        "POSN43312W123174,EASON,215754,370,EBINY,220601,ELENN,M48,02216,185/TS215754,0921227A40",
    ),
    (
        "H1",
        "POSN45209W122550,PEGTY,220309,134,MINNE,220424,HISKU,M6,060013,269,366,355K,292K,730A5B",
    ),
    (
        "H1",
        "FPN/FNUAL1187/RP:DA:KSFO:AA:KPHX:F:KAYEX,N36292W120569..LOSHN,N35509W120000..BOILE,N34253W118016..BLH,N33358W114457DDFB",
    ),
    (
        "H1",
        "FPN/RI:DA:KEWR:AA:KDFW:CR:EWRDFW01(17L)..SAAME.J6.HVQ.Q68.LITTR..MEEOW..FEWWW:A:SEEVR4.FEWWW:F:VECTOR..DISCO..RIVET:AP:ILS 17L.RIVET:F:TACKEC8B5",
    ),
    (
        "H1",
        "FPN/FNAAL1956/RP:DA:KPHL:AA:KPHX:CR:PHLPHX61:R:27L(26O):D:PHL3:A:EAGUL6.ZUN:AP:ILS26..AIR,N40010W080490.J110.BOWRR..VLA,N39056W089097..STL,N38516W090289..GIBSN,N38430W092244..TYGER,N38410W094050..GCK,N37551W100435..DIXAN,N36169W105573..ZUN,N34579W109093293B",
    ),
    (
        "H1",
        "FPN/SN2125/FNQFA780/RI:DA:YPPH:CR:PERMEL001:AA:YMML..MEMUP,S33451E\r\n120525.Y53.WENDY0560",
    ),
    ("H1", "FPN/"),
    ("H1", "PLAIN TEXT"),
    (
        "20",
        "POSN38160W077075,,211733,360,OTT,212041,,N42,19689,40,544",
    ),
    ("20", "POSN32249E045047,,082806,380,DEBNI"),
    ("20", "RST0000000000000000000KORDKSFO"),
    ("20", "RST something"),
    (
        "4J",
        "POS/ID91459S,BANKR31,/DC03032024,142813/MR64,0/ET31539/PSN39277W077359,142800,240,N39300W077110,031430,N38560W077150,M28,27619,MT370/CG311,160,350/FB732/VR329071",
    ),
    (
        "4J",
        "4J01 POSWX 0318/20 ETAD/ETAD .00318S\r\n/POS N5043.5E01121.8/OVR 0817\r\n/ALT 270/TFW 1342/TAS 490/SAT -032\r\n/POS GOVEN /OVR 0835\r\n/POS DILVI\r\n/WND 334060/TRB /SKY DCC3",
    ),
    ("4J", "/WND 999/SAT M48/TAS X/ALT FL"),
    ("4J", "no position here"),
    ("Q0", ""),
    ("Q1", "KEWR1200121513101330    KATL"),
    ("Q2", "KSFO0830"),
    ("Q2", "   2002  99/DS KJFK"),
    ("QA", "KEWR1200"),
    ("QB", "KEWR1215"),
    ("QC", "KEWR1310"),
    ("QD", "KEWR1330"),
    ("QE", "KEWR1200KATL"),
    ("QF", "KEWR2210KATL"),
    ("QF", "EWR2210ATL"),
    ("QG", "KEWR12001330"),
    ("QH", "KEWR1200"),
    ("QK", "KEWR1310KATL"),
    ("QL", "KATL    1330 KEWR"),
    ("QM", "KATL    KEWR"),
    ("QN", "    KATL0830"),
    ("QP", "KLAXKJFK1305"),
    ("QQ", "KEWRKSWF20041942"),
    ("QQ", "KEWRKDFW1829OS KDFW /FUL0306/MO 1816/APH 0000000"),
    ("QR", "KLAXKJFK2210"),
    ("QS", "KLAXKJFK2247"),
    ("QT", "KLAXKJFK13052247"),
    ("QX", ""),
    ("Q8", "KEWR1200"),
    ("10", "ARR01       KDFW0845"),
    ("11", "             /DS KJFK/ETA 0930"),
    ("12", "KEWR,KATL"),
    ("1G", "KEWR,KATL"),
    ("83", "KEWR,KATL"),
    ("15", "FST01KEWRKATL"),
    ("17", "ETA 0930,KEWR,KATL"),
    ("21", "ABCDEF,KEWR,KATL"),
    ("2N", "TKO01ABCDEF/ABCDEFGHKEWRKATL"),
    ("2Z", "KATL"),
    ("33", ",ABCDEFGHIJKLMNOPQRS,KEWR,KATL"),
    ("39", "GTA01ABCDEFGHIJ/ABCDEFGHKEWRKATL"),
    ("45", "AKATL"),
    ("80", "ABCDEF/DEST/KATL"),
    ("8D", "ABCD,ABCDEFGHIJKLMNOPQRSTUVWXYZABCD,KEWR,KATL"),
    ("8E", "KATL,0930"),
    ("8S", "KATL,2561"),
    ("SA", "0EV121314VS/EXTRA"),
    ("SA", "0L2235959V"),
    ("SA", "0EQ121314V"),
    ("SA", "0EV256060V"),
    ("SA", "0EV121314V/"),
    ("MA", "F012000345"),
    ("MA", "S012003abc"),
    ("MA", "K0120"),
    ("MA", "K012G"),
    ("MA", "A0123"),
    ("MA", "Y012"),
    ("MA", "X"),
    ("MA", "T00|"),
    ("MA", "not miam"),
    ("5Z", "/TXT\r\nDID U GET THE TIMES"),
    ("5Z", "/B3 DCAORD 14 R27C"),
    ("5Z", "/B3 ATLIAD 14 R1C G1273"),
    ("5Z", "/C3 ATLIAD"),
    (
        "5Z",
        "/C6 ORDCHS CHS HI...NO APU TONIGHT\r\nWILL NEED GROUND PWR",
    ),
    ("5Z", "/ZZ SOMETHING"),
    ("5Z", "not a 5z message"),
    ("_d", ""),
    ("H1", ""),
];

const FUZZ_LABELS: &[&str] = &[
    "H1", "B6", "AA", "BA", "A6", "MA", "SA", "5Z", "20", "4J", "Q1", "QF", "QQ", "10", "11", "17",
    "8D", "80", "_d", "Q0",
];

const FUZZ_PIECES: &[&str] = &[
    "#DFB",
    "#CFB",
    "- #",
    "/M1 ",
    "POS",
    "N4331",
    "W12317",
    "E0450",
    ",",
    "/",
    ".",
    "FPN/",
    "RP",
    "RI",
    ":DA:",
    "KSFO",
    ":AA:",
    ":F:",
    "..",
    "OHMA",
    "/RTNBOCR.",
    ".ADS.",
    ".AT1.",
    ".DIS.",
    "BOMASAI",
    "VT-ANB",
    "0724D9586A36C92B",
    "FFFF",
    "0EV",
    "121314",
    "VS",
    "T0",
    "F012",
    "S012",
    "/TXT",
    "\r\n",
    "/B3 ",
    "DCAORD",
    " 14 R",
    "27C",
    "WND ",
    "334060",
    "SAT ",
    "M48",
    "ALT ",
    "270",
    "TAS 490",
    "ARR01",
    "ETA ",
    "0930",
    "/DS ",
    "/ETA ",
    "FST01",
    "RST",
    "TKO01",
    "GTA01",
    "A",
    "/DEST",
    " ",
    "z",
    "|",
    "-",
    "9",
];

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn assert_parse_matches(octets: &[u8]) {
    let ours = block::parse(octets).map(|b| to_json(&b));
    let theirs = xng_acars::block::parse(octets).map(|b| to_json(&b));
    assert_eq!(ours, theirs, "block {octets:02x?}");
}

fn assert_decode_matches(label: &str, text: &str, downlink: bool) {
    let ours = decode(label, text, downlink);
    let theirs = xng_acars::decode(label, text, downlink);
    assert_eq!(
        to_json(&ours),
        to_json(&theirs),
        "decode {label} {text:?} downlink={downlink}"
    );
    assert_eq!(
        ours.app.as_ref().and_then(summary),
        theirs.app.as_ref().and_then(xng_acars::summary),
        "summary {label} {text:?}"
    );
}

struct BlockArgs<'a> {
    tail: &'a str,
    ack: Option<char>,
    label: &'a str,
    block_id: char,
    msg_num: Option<&'a str>,
    flight: Option<&'a str>,
    text: &'a str,
    etb: bool,
}

fn build_both(a: &BlockArgs) -> Vec<u8> {
    let ours = block::build(
        '2', a.tail, a.ack, a.label, a.block_id, a.msg_num, a.flight, a.text, a.etb,
    );
    let theirs = xng_acars::block::build(
        '2', a.tail, a.ack, a.label, a.block_id, a.msg_num, a.flight, a.text, a.etb,
    );
    assert_eq!(ours, theirs, "build {}", a.text);
    ours
}

fn variants<'a>(label: &'a str, text: &'a str) -> [BlockArgs<'a>; 4] {
    [
        BlockArgs {
            tail: "N123AB",
            ack: None,
            label,
            block_id: '2',
            msg_num: Some("M01A"),
            flight: Some("UA0001"),
            text,
            etb: false,
        },
        BlockArgs {
            tail: ".VT-ANB",
            ack: Some('3'),
            label,
            block_id: 'A',
            msg_num: None,
            flight: None,
            text,
            etb: false,
        },
        BlockArgs {
            tail: "D-AIBC",
            ack: None,
            label,
            block_id: '5',
            msg_num: Some("D65C"),
            flight: Some("AF7728"),
            text,
            etb: true,
        },
        BlockArgs {
            tail: "",
            ack: Some('X'),
            label,
            block_id: 'K',
            msg_num: None,
            flight: None,
            text,
            etb: true,
        },
    ]
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn ohma_text(prefix: &str, json: &str) -> String {
    let packed = codec::testing::zlib(json.as_bytes());
    format!("{prefix}OHMA{}", codec::testing::base64_encode(&packed))
}

fn generated_samples() -> Vec<(&'static str, String)> {
    vec![
        (
            "H1",
            ohma_text("", r#"{"version":1,"message":{"sysid":"APU"}}"#),
        ),
        ("H1", ohma_text("/RTNBOCR.", r#"{"a":2}"#)),
        ("H1", ohma_text("- #MD/O2.", r#"{"b":[1,2,3]}"#)),
        ("H1", ohma_text("", r#"{"version":7}"#)),
        ("H1", ohma_text("", "not json")),
        (
            "MA",
            miam::testing::deflated_data_pdu(b"#T2BThis is the embedded MIAM payload"),
        ),
        ("MA", miam::testing::deflated_data_pdu(&[0, 1, 2, 255])),
        ("MA", miam::testing::plain_data_pdu("HELLO FILE WORLD")),
    ]
}

#[test]
fn decode_matches_xng_on_every_sample() {
    for &(label, text) in SAMPLES {
        for downlink in [true, false] {
            assert_decode_matches(label, text, downlink);
        }
    }
    for (label, text) in generated_samples() {
        for downlink in [true, false] {
            assert_decode_matches(label, &text, downlink);
        }
    }
}

#[test]
fn blocks_build_and_parse_like_xng() {
    let generated = generated_samples();
    let all = SAMPLES.iter().map(|&(label, text)| (label, text)).chain(
        generated
            .iter()
            .map(|(label, text)| (*label, text.as_str())),
    );
    for (label, text) in all {
        for args in variants(label, text) {
            assert_parse_matches(&build_both(&args));
        }
    }
}

#[test]
fn corrupted_blocks_parse_like_xng() {
    for &(label, text) in SAMPLES.iter().step_by(7) {
        for args in variants(label, text) {
            let good = build_both(&args);
            for at in 0..good.len() {
                for mask in [0x01u8, 0x80, 0x7F] {
                    let mut bad = good.clone();
                    bad[at] ^= mask;
                    assert_parse_matches(&bad);
                }
                assert_parse_matches(&good[..at]);
            }
        }
    }
}

#[test]
fn raw_octets_parse_like_xng() {
    let mut rng = XorShift(0x5eed_acab_0bad_f00d);
    for _ in 0..20_000 {
        let len = rng.below(48);
        let mut octets: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if let Some(first) = octets.first_mut() {
            *first = 0x01;
        }
        if octets.len() > 13 && rng.below(2) == 0 {
            octets[13] = [0x02, 0x03, 0x17, 0x83][rng.below(4)];
        }
        assert_parse_matches(&octets);
    }
}

#[test]
fn fuzzed_texts_decode_like_xng() {
    let mut rng = XorShift(0x0123_4567_89ab_cdef);
    for _ in 0..20_000 {
        let label = FUZZ_LABELS[rng.below(FUZZ_LABELS.len())];
        let text: String = (0..rng.below(12))
            .map(|_| FUZZ_PIECES[rng.below(FUZZ_PIECES.len())])
            .collect();
        let downlink = rng.below(2) == 0;
        assert_decode_matches(label, &text, downlink);
        let args = BlockArgs {
            tail: "N1",
            ack: None,
            label,
            block_id: if downlink { '3' } else { 'C' },
            msg_num: downlink.then_some("M02B"),
            flight: downlink.then_some("XX0001"),
            text: &text,
            etb: false,
        };
        assert_parse_matches(&build_both(&args));
    }
}

#[test]
fn ohma_and_miam_edge_cases_match_xng() {
    let packed = codec::testing::zlib(br#"{"x":1}"#);
    let b64 = codec::testing::base64_encode(&packed);
    let unpadded = b64.trim_end_matches('=').to_owned();
    let truncated = codec::testing::base64_encode(&packed[..packed.len() - 3]);
    let raw_deflate = codec::testing::base64_encode(&codec::testing::deflate_raw(b"{}"));
    let ohma_inputs = [
        format!("OHMA{b64}"),
        format!("OHMA{unpadded}"),
        format!("OHMA{b64}="),
        format!("OHMA{b64}AAAA"),
        format!("OHMA{truncated}"),
        format!("OHMA{raw_deflate}"),
        format!("OHMA{}", &b64[..b64.len() - 1]),
        "OHMA".to_owned(),
        "OHMATR==".to_owned(),
        "OHMATQ==".to_owned(),
        "OHMAOHMA".to_owned(),
        "/O2.RYKO".to_owned(),
        "/RTNBOCR.OHMA/RTNBOCR.OHMA".to_owned(),
    ];
    for text in &ohma_inputs {
        assert_eq!(ohma::parse(text), xng_acars::ohma::parse(text), "{text}");
    }
    let body = codec::testing::deflate_raw(b"payload text");
    let miam_inputs = [
        format!("T00{}|{}", miam::testing::base85_encode(&[0x01; 24]), "abc"),
        format!("T-0{}|raw body", miam::testing::base85_encode(&[0x02; 8])),
        format!(".0{}|", miam::testing::base85_encode(&[0x12; 8])),
        format!("T00z|{}", miam::testing::base85_encode(&body)),
        "T00~~~~~|".to_owned(),
        "T0".to_owned(),
    ];
    for text in &miam_inputs {
        assert_eq!(
            miam::parse(text).map(|f| to_json(&f)),
            xng_acars::miam::parse(text).map(|f| to_json(&f)),
            "{text}"
        );
    }
}

#[test]
fn downlink_min_split_matches_xng() {
    for raw in ["M01A", "M07C", "D5R2", "S01.", "AAAZ", "M0", "", "ABCDEF"] {
        assert_eq!(
            min::split_downlink(raw).map(|m| to_json(&m)),
            xng_acars::min::split_downlink(raw).map(|m| to_json(&m)),
            "{raw}"
        );
    }
}

fn core(tail: &str, block_id: char, msg_num: Option<&str>, text: &str, more: bool) -> AcarsCore {
    AcarsCore {
        mode: '2',
        tail: Some(tail.into()),
        label: "H1".into(),
        block_id: Some(block_id),
        msg_num: msg_num.map(Into::into),
        text: text.into(),
        more_to_come: more,
        ..AcarsCore::default()
    }
}

#[test]
fn reassembly_matches_xng() {
    let steps = [
        (core("N1", '2', Some("M01A"), "HELLO", false), 0.0),
        (core("N2", '2', Some("M07A"), "FIRST-", true), 1.0),
        (core("N2", '2', Some("M07A"), "FIRST-", true), 2.0),
        (core("N2", '3', Some("M07B"), "SECOND", false), 3.0),
        (core("N3", '2', Some("M09A"), "A", true), 4.0),
        (core("N3", '4', Some("M09C"), "C", false), 5.0),
        (core("N4", 'A', None, "/O2.OHMAabcd", true), 6.0),
        (core("N4", 'B', None, "efgh", false), 7.0),
        (core("N5", 'X', None, "", false), 8.0),
        (core("N6", '2', Some("M11A"), "OLD-", true), 9.0),
        (core("N6", '3', Some("M11B"), "LATE", false), 500.0),
        (core("N7", '2', Some("M1"), "SHORT", true), 501.0),
        (core("N8", '2', Some("M12."), "DOT", true), 502.0),
    ];
    let mut ours = Reassembler::new(120.0);
    let mut theirs = xng_acars::reasm::Reassembler::new(120.0);
    for (core, now) in &steps {
        let xng_core: xng_types::AcarsCore = serde_json::from_value(to_json(core)).unwrap();
        assert_eq!(to_json(core), to_json(&xng_core));
        let a: Reasm = ours.push(core, *now);
        let b = theirs.push(&xng_core, *now);
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "{core:?}");
        assert_eq!(a.assstat(), b.assstat());
    }
}
