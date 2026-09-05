use super::*;

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

#[must_use]
pub fn typescript_frames() -> String {
    let mut out = format!(
        "export const PROTOCOL_VERSION = {PROTOCOL_VERSION};\nconst HEADER_LEN = {HEADER_LEN};\n"
    );
    for (name, kind) in [
        ("SPECTRUM", FrameKind::Spectrum),
        ("AUDIO_OPUS", FrameKind::AudioOpus),
        ("IQ_F32", FrameKind::IqF32),
        ("VIDEO_GRAY", FrameKind::VideoGray),
        ("VIDEO_RGB", FrameKind::VideoRgb),
        ("SYMBOLS", FrameKind::Symbols),
        ("RANGE_DOPPLER", FrameKind::RangeDoppler),
    ] {
        out.push_str(&format!(
            "export const FRAME_KIND_{name} = {};\n",
            kind as u8
        ));
    }
    out.push_str(include_str!("helpers.ts"));
    for (name, kinds, fields) in [
        ("Spectrum", "FRAME_KIND_SPECTRUM", SpectrumFrame::fields()),
        ("Audio", "FRAME_KIND_AUDIO_OPUS", AudioFrame::fields()),
        ("Iq", "FRAME_KIND_IQ_F32", IqFrame::fields()),
        ("Symbols", "FRAME_KIND_SYMBOLS", SymbolFrame::fields()),
        (
            "RangeDoppler",
            "FRAME_KIND_RANGE_DOPPLER",
            RangeDopplerFrame::fields(),
        ),
        (
            "Video",
            "FRAME_KIND_VIDEO_GRAY, FRAME_KIND_VIDEO_RGB",
            VideoFrame::fields(),
        ),
    ] {
        emit(&mut out, name, kinds, fields);
    }
    out
}

fn emit(out: &mut String, name: &str, kinds: &str, fields: &[(&str, &str)]) {
    let interface = if name == "Symbols" { "Symbol" } else { name };
    out.push_str(&format!(
        "export interface {interface}Frame {{\n streamId: number; seq: number; timestamp: bigint;\n"
    ));
    for &(field, ty) in fields {
        let field = camel(field);
        let ts = match ty {
            "bytes" | "bytes16" => "Uint8Array",
            "floats" | "floats16" => "Float32Array",
            "plane" => "SymbolPlane",
            "video" => {
                out.push_str("format: \"gray\" | \"rgb\"; pixels: Uint8Array;\n");
                continue;
            }
            _ => "number",
        };
        out.push_str(&format!("{field}: {ts};\n"));
    }
    out.push_str(&format!("}}\nexport function decode{name}(buffer: ArrayBuffer): {interface}Frame | null {{\nconst reader = new FrameReader(buffer, [{kinds}]);\nif (!reader.valid) return null;\nconst streamId = reader.u16();\nconst seq = reader.u32();\nconst timestamp = reader.u64();\n"));
    let mut result = vec![
        "streamId".to_string(),
        "seq".to_string(),
        "timestamp".to_string(),
    ];
    for &(field, ty) in fields {
        let field = camel(field);
        if ty == "video" {
            out.push_str("const format = frameKind(buffer) === FRAME_KIND_VIDEO_RGB ? \"rgb\" : \"gray\";\nconst pixels = reader.bytes(width * height * (format === \"rgb\" ? 3 : 1));\nif (pixels.length === 0) return null;\n");
            result.extend(["format".into(), "pixels".into()]);
            continue;
        }
        let expression = match ty {
            "bytes16" => "reader.bytes(reader.u16())".to_string(),
            "floats16" => "reader.floats(reader.u16())".to_string(),
            "bytes" if field == "cells" => "reader.bytes(ranges * dopplers)".to_string(),
            "bytes" => "reader.bytes()".to_string(),
            "floats" => "reader.floats()".to_string(),
            "plane" => "reader.plane()".to_string(),
            _ => format!("reader.{ty}()"),
        };
        out.push_str(&format!("const {field} = {expression};\n"));
        if field == "samples" {
            out.push_str("if (samples.length === 0 || samples.length % 2 !== 0) return null;\n");
        }
        if field == "cells" {
            out.push_str("if (cells.length === 0) return null;\n");
        }
        result.push(field);
    }
    out.push_str(&format!(
        "if (!reader.complete) return null;\nreturn {{ {} }};\n}}\n",
        result.join(", ")
    ));
}
