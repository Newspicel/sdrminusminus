use super::*;

struct Emitted {
    name: &'static str,
    kinds: &'static str,
    fields: &'static [(&'static str, &'static str)],
    cells: Option<&'static str>,
    limit: Option<&'static str>,
}

fn camel(name: &str) -> String {
    let mut upper = false;
    name.chars()
        .filter_map(|c| {
            if c == '_' {
                upper = true;
                None
            } else if upper {
                upper = false;
                Some(c.to_ascii_uppercase())
            } else {
                Some(c)
            }
        })
        .collect()
}

const KINDS: [(&str, FrameKind); 10] = [
    ("SPECTRUM", FrameKind::Spectrum),
    ("AUDIO_OPUS", FrameKind::AudioOpus),
    ("IQ_F32", FrameKind::IqF32),
    ("VIDEO_GRAY", FrameKind::VideoGray),
    ("VIDEO_RGB", FrameKind::VideoRgb),
    ("SYMBOLS", FrameKind::Symbols),
    ("RANGE_DOPPLER", FrameKind::RangeDoppler),
    ("SPATIAL_SPECTRUM", FrameKind::SpatialSpectrum),
    ("VISIBILITY", FrameKind::Visibility),
    ("FUSION_GRID", FrameKind::FusionGrid),
];

fn emitted() -> [Emitted; 9] {
    let plain = |name, kinds, fields| Emitted {
        name,
        kinds,
        fields,
        cells: None,
        limit: None,
    };
    let shaped = |name, kinds, fields, cells| Emitted {
        name,
        kinds,
        fields,
        cells: Some(cells),
        limit: None,
    };
    [
        plain("Spectrum", "FRAME_KIND_SPECTRUM", SpectrumFrame::fields()),
        plain("Audio", "FRAME_KIND_AUDIO_OPUS", AudioFrame::fields()),
        plain("Iq", "FRAME_KIND_IQ_F32", IqFrame::fields()),
        plain("Symbols", "FRAME_KIND_SYMBOLS", SymbolFrame::fields()),
        shaped(
            "RangeDoppler",
            "FRAME_KIND_RANGE_DOPPLER",
            RangeDopplerFrame::fields(),
            "ranges * dopplers",
        ),
        plain(
            "Video",
            "FRAME_KIND_VIDEO_GRAY, FRAME_KIND_VIDEO_RGB",
            VideoFrame::fields(),
        ),
        shaped(
            "SpatialSpectrum",
            "FRAME_KIND_SPATIAL_SPECTRUM",
            SpatialSpectrumFrame::fields(),
            "bearings * bins",
        ),
        shaped(
            "Visibility",
            "FRAME_KIND_VISIBILITY",
            VisibilityFrame::fields(),
            "baselines * bins",
        ),
        Emitted {
            limit: Some("FUSION_FRAME_CELLS"),
            ..shaped(
                "FusionGrid",
                "FRAME_KIND_FUSION_GRID",
                FusionGridFrame::fields(),
                "cols * rows",
            )
        },
    ]
}

#[must_use]
pub fn typescript_frames() -> String {
    let mut out = format!(
        "export const PROTOCOL_VERSION = {PROTOCOL_VERSION};\nconst HEADER_LEN = {HEADER_LEN};\nexport const FUSION_FRAME_CELLS = {FUSION_FRAME_CELLS};\n"
    );
    out.push_str(&format!(
        "export const WS_SUBPROTOCOL = {:?};\nexport const WS_BEARER_PROTOCOL_PREFIX = {:?};\n",
        crate::WS_SUBPROTOCOL,
        crate::WS_BEARER_PROTOCOL_PREFIX
    ));
    for (name, kind) in KINDS {
        out.push_str(&format!(
            "export const FRAME_KIND_{name} = {};\n",
            kind as u8
        ));
    }
    out.push_str(include_str!("helpers.ts"));
    for frame in emitted() {
        emit(&mut out, &frame);
    }
    out
}

fn ts_type(ty: &str) -> &'static str {
    match ty {
        "bytes" | "bytes16" => "Uint8Array",
        "floats" | "floats16" => "Float32Array",
        "plane" => "SymbolPlane",
        _ => "number",
    }
}

fn emit_interface(out: &mut String, interface: &str, frame: &Emitted) {
    out.push_str(&format!(
        "export interface {interface}Frame {{\n streamId: number; seq: number; timestamp: bigint;\n"
    ));
    for &(field, ty) in frame.fields {
        if ty == "video" {
            out.push_str("format: \"gray\" | \"rgb\"; pixels: Uint8Array;\n");
        } else {
            out.push_str(&format!("{}: {};\n", camel(field), ts_type(ty)));
        }
    }
    out.push_str("}\n");
}

fn expression(ty: &str, cells: Option<&str>) -> String {
    match (ty, cells) {
        ("bytes16", _) => "reader.bytes(reader.u16())".to_string(),
        ("floats16", _) => "reader.floats(reader.u16())".to_string(),
        ("bytes", Some(count)) => format!("reader.bytes({count})"),
        ("bytes", None) => "reader.bytes()".to_string(),
        ("floats", _) => "reader.floats()".to_string(),
        ("plane", _) => "reader.plane()".to_string(),
        (scalar, _) => format!("reader.{scalar}()"),
    }
}

fn emit(out: &mut String, frame: &Emitted) {
    let name = frame.name;
    let interface = if name == "Symbols" { "Symbol" } else { name };
    emit_interface(out, interface, frame);
    out.push_str(&format!("export function decode{name}(buffer: ArrayBuffer): {interface}Frame | null {{\nconst reader = new FrameReader(buffer, [{}]);\nif (!reader.valid) return null;\nconst streamId = reader.u16();\nconst seq = reader.u32();\nconst timestamp = reader.u64();\n", frame.kinds));
    let mut result = vec![
        "streamId".to_string(),
        "seq".to_string(),
        "timestamp".to_string(),
    ];
    for &(field, ty) in frame.fields {
        let field = camel(field);
        if ty == "video" {
            out.push_str("const format = frameKind(buffer) === FRAME_KIND_VIDEO_RGB ? \"rgb\" : \"gray\";\nconst pixels = reader.bytes(width * height * (format === \"rgb\" ? 3 : 1));\nif (pixels.length === 0) return null;\n");
            result.extend(["format".into(), "pixels".into()]);
            continue;
        }
        out.push_str(&format!(
            "const {field} = {};\n",
            expression(ty, frame.cells)
        ));
        if field == "samples" {
            out.push_str("if (samples.length === 0 || samples.length % 2 !== 0) return null;\n");
        }
        if let Some(count) = frame.cells
            && matches!(ty, "bytes" | "bytes16")
        {
            out.push_str(&format!(
                "if ({field}.length === 0 || {field}.length !== {count}) return null;\n"
            ));
        }
        result.push(field);
    }
    if let Some(limit) = frame.limit {
        out.push_str(&format!(
            "if (cols > {limit} || rows > {limit}) return null;\n"
        ));
    }
    out.push_str(&format!(
        "if (!reader.complete) return null;\nreturn {{ {} }};\n}}\n",
        result.join(", ")
    ));
}
