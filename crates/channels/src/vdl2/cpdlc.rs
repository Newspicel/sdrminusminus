use serde_json::{Value, json};

use super::cpdlc_tables::{DOWNLINK_ELEMENTS, UPLINK_ELEMENTS};

struct Per<'a> {
    bits: &'a [u8],
    pos: usize,
}

impl<'a> Per<'a> {
    fn new(bytes: &'a [u8], store: &'a mut Vec<u8>) -> Per<'a> {
        store.clear();
        store.extend(
            bytes
                .iter()
                .flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1)),
        );
        Per {
            bits: store,
            pos: 0,
        }
    }

    fn bit(&mut self) -> Option<u8> {
        let b = *self.bits.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn uint(&mut self, n: usize) -> Option<u64> {
        if self.pos + n > self.bits.len() {
            return None;
        }
        let v = self.bits[self.pos..self.pos + n]
            .iter()
            .fold(0u64, |v, &b| (v << 1) | b as u64);
        self.pos += n;
        Some(v)
    }

    fn constrained(&mut self, lo: i64, hi: i64) -> Option<i64> {
        let range = (hi - lo + 1) as u64;
        if range == 1 {
            return Some(lo);
        }
        let bits = 64 - (range - 1).leading_zeros() as usize;
        Some(lo + self.uint(bits)? as i64)
    }

    fn length_frag(&mut self) -> Option<(usize, bool)> {
        if self.bit()? == 0 {
            return Some((self.uint(7)? as usize, false));
        }
        if self.bit()? == 0 {
            return Some((self.uint(14)? as usize, false));
        }
        let m = self.uint(6)? as usize;
        if m == 0 || m > 4 {
            return None;
        }
        Some((m * 16384, true))
    }

    fn length(&mut self) -> Option<usize> {
        let (mut total, mut more) = self.length_frag()?;
        while more {
            let (n, m) = self.length_frag()?;
            total = total.checked_add(n)?;
            more = m;
        }
        Some(total)
    }

    fn normally_small(&mut self) -> Option<u64> {
        if self.bit()? == 0 {
            return self.uint(6);
        }
        let n = self.length()?;
        let bytes = self.remaining_bytes(n * 8)?;
        Some(bytes.iter().fold(0u64, |v, &b| (v << 8) | b as u64))
    }

    fn ia5(&mut self, min: i64, max: i64) -> Option<String> {
        let n = self.constrained(min, max)? as usize;
        let mut s = String::with_capacity(n);
        for _ in 0..n {
            s.push(self.uint(7)? as u8 as char);
        }
        Some(s)
    }

    fn remaining_bytes(&mut self, nbits: usize) -> Option<Vec<u8>> {
        if self.pos + nbits > self.bits.len() {
            return None;
        }
        let out = self.bits[self.pos..self.pos + nbits]
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |v, (i, &b)| v | (b << (7 - i)))
            })
            .collect();
        self.pos += nbits;
        Some(out)
    }
}

pub fn parse_apdu(bytes: &[u8]) -> Option<Value> {
    parse_pdus(bytes, true).or_else(|| parse_pdus(bytes, false))
}

#[cfg(test)]
pub(crate) fn build_downlink_wilco_for_test() -> Vec<u8> {
    fn push(bits: &mut Vec<u8>, v: u64, n: usize) {
        for k in (0..n).rev() {
            bits.push(((v >> k) & 1) as u8);
        }
    }
    let mut m = Vec::new();
    push(&mut m, 0, 1);
    push(&mut m, 0, 1);
    push(&mut m, 12, 6);
    push(&mut m, (2026 - 1996) as u64, 7);
    push(&mut m, 6 - 1, 4);
    push(&mut m, 11 - 1, 5);
    push(&mut m, 1, 5);
    push(&mut m, 22, 6);
    push(&mut m, 33, 6);
    push(&mut m, 0, 1);
    push(&mut m, 0, 3);
    push(&mut m, 0, 7);
    let inner_bits = m.len();
    let mut o = Vec::new();
    push(&mut o, 0, 1);
    push(&mut o, 3, 2);
    push(&mut o, 0, 1);
    push(&mut o, 0, 1);
    push(&mut o, 1, 1);
    push(&mut o, 0, 1);
    push(&mut o, inner_bits as u64, 7);
    o.extend(&m);
    push(&mut o, 0, 1);
    push(&mut o, 0, 7);
    o.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |v, (i, &b)| v | (b << (7 - i)))
        })
        .collect()
}

#[cfg(test)]
pub fn parse_acse_apdu(bytes: &[u8]) -> Option<Value> {
    let mut store = Vec::new();
    let mut p = Per::new(bytes, &mut store);
    if p.bit()? != 0 {
        return Some(json!({
            "application": "ACSE",
            "pdu": "extension-alternative",
            "note": "ACSE-apdu CHOICE extension addition; body undecoded",
        }));
    }
    let idx = p.uint(3)?;
    let kind = match idx {
        0 => "aarq",
        1 => "aare",
        2 => "rlrq",
        3 => "rlre",
        4 => "abrt",
        _ => return None,
    };
    let mut out = json!({ "application": "ACSE", "pdu": kind });
    match idx {
        0 | 1 => {
            out["note"] = json!(
                "AARQ/AARE recognised; body (application-context-name OID, \
                 AP-title, user-information EXTERNAL) deferred"
            );
        }
        2 => read_acse_release(&mut p, &mut out, true),
        3 => read_acse_release(&mut p, &mut out, false),
        4 => read_acse_abrt(&mut p, &mut out),
        _ => {}
    }
    Some(out)
}

#[cfg(test)]
fn read_acse_release(p: &mut Per, out: &mut Value, request: bool) {
    let Some(ext) = p.bit() else { return };
    if ext != 0 {
        out["note"] = json!("RLRQ/RLRE extension additions present; reason deferred");
        return;
    }
    let Some(has_reason) = p.bit() else { return };
    if has_reason == 0 {
        out["reason"] = json!("absent");
        return;
    }
    let Some(rext) = p.bit() else { return };
    if rext != 0 {
        out["reason"] = json!("(extended)");
        return;
    }
    let Some(v) = p.uint(5) else { return };
    out["reason"] = json!(v);
    let name = if request {
        match v {
            0 => Some("normal"),
            1 => Some("urgent"),
            30 => Some("user-defined"),
            _ => None,
        }
    } else {
        match v {
            0 => Some("normal"),
            1 => Some("not-finished"),
            30 => Some("user-defined"),
            _ => None,
        }
    };
    if let Some(n) = name {
        out["reason_text"] = json!(n);
    }
}

#[cfg(test)]
fn read_acse_abrt(p: &mut Per, out: &mut Value) {
    let Some(ext) = p.bit() else { return };
    if ext != 0 {
        out["note"] = json!("ABRT extension additions present; body deferred");
        return;
    }
    let Some(has_diag) = p.bit() else { return };
    let Some(sext) = p.bit() else { return };
    if sext == 0 {
        if let Some(s) = p.bit() {
            out["abort_source"] = json!(match s {
                0 => "acse-service-user",
                _ => "acse-service-provider",
            });
        }
    } else {
        out["abort_source"] = json!("(extended)");
    }
    if has_diag == 1 {
        let Some(dext) = p.bit() else { return };
        if dext == 0 {
            if let Some(d) = p.uint(3) {
                const D: [&str; 6] = [
                    "no-reason-given",
                    "protocol-error",
                    "authentication-mechanism-name-not-recognized",
                    "authentication-mechanism-name-required",
                    "authentication-failure",
                    "authentication-required",
                ];
                out["abort_diagnostic"] = json!(D.get(d as usize).copied().unwrap_or("?"));
            }
        } else {
            out["abort_diagnostic"] = json!("(extended)");
        }
    }
}

fn parse_pdus(bytes: &[u8], downlink: bool) -> Option<Value> {
    let mut store = Vec::new();
    let mut p = Per::new(bytes, &mut store);
    let extended = p.bit()? != 0;
    let (alts, idx_bits) = if downlink { (4u64, 2) } else { (6u64, 3) };
    if extended {
        let ext_idx = p.normally_small()?;
        return Some(json!({
            "application": "CPDLC",
            "version": "ATN-B1",
            "direction": if downlink { "downlink" } else { "uplink" },
            "pdu": "extension-alternative",
            "extension_index": ext_idx,
            "note": "CHOICE extension addition (no root alternative); body undecoded",
        }));
    }
    let idx = p.uint(idx_bits)?;
    if idx >= alts {
        return None;
    }
    let kind = match (downlink, idx) {
        (_, 0) => "abort-user",
        (_, 1) => "abort-provider",
        (true, 2) => "startdown",
        (true, 3) => "send",
        (false, 2) => "startup",
        (false, 3) => "send",
        (false, 4) => "forward",
        (false, 5) => "forward-response",
        _ => return None,
    };
    let mut out = json!({
        "application": "CPDLC",
        "version": "ATN-B1",
        "direction": if downlink { "downlink" } else { "uplink" },
        "pdu": kind,
    });
    match kind {
        "send" | "startup" => {
            out["message"] = protected_message(&mut p, downlink)?;
        }
        "startdown" => {
            if p.bit()? == 1 {
                out["mode"] = json!(if p.bit()? == 1 { "dsc" } else { "cpdlc" });
            }
            out["message"] = protected_message(&mut p, downlink)?;
        }
        "abort-user" => {
            if p.bit()? == 0 {
                let r = p.constrained(0, 12)?;
                out["reason"] = json!(r);
                if let Some(t) = pm_user_abort_reason(r) {
                    out["reason_text"] = json!(t);
                }
            } else {
                out["reason"] = json!("(extended)");
            }
        }
        "abort-provider" => {
            if p.bit()? == 0 {
                let r = p.constrained(0, 7)?;
                out["reason"] = json!(r);
                if let Some(t) = pm_provider_abort_reason(r) {
                    out["reason_text"] = json!(t);
                }
            } else {
                out["reason"] = json!("(extended)");
            }
        }
        "forward-response" => {
            if p.bit()? == 0 {
                out["response"] = json!(atc_forward_response(p.constrained(0, 2)?));
            } else {
                out["response"] = json!("(extended)");
            }
        }
        "forward" => {
            if let Some(fwd) = read_atc_forward_message(&mut p, downlink) {
                out["forward"] = fwd;
            }
        }
        _ => {}
    }
    Some(out)
}

fn pm_user_abort_reason(r: i64) -> Option<&'static str> {
    const R: [&str; 13] = [
        "undefined",
        "no-message-identification-numbers-available",
        "duplicate-message-identification-numbers",
        "no-longer-next-data-authority",
        "current-data-authority-abort",
        "commanded-termination",
        "invalid-response",
        "time-out-of-synchronisation",
        "unknown-integrity-check",
        "validation-failure",
        "unable-to-decode-message",
        "invalid-pdu",
        "invalid-CPDLC-message",
    ];
    R.get(r as usize).copied()
}

fn pm_provider_abort_reason(r: i64) -> Option<&'static str> {
    const R: [&str; 8] = [
        "timer-expired",
        "undefined-error",
        "invalid-PDU",
        "protocol-error",
        "communication-service-error",
        "communication-service-failure",
        "invalid-QOS-parameter",
        "expected-PDU-missing",
    ];
    R.get(r as usize).copied()
}

fn atc_forward_response(r: i64) -> &'static str {
    match r {
        0 => "success",
        1 => "service-not-supported",
        2 => "version-not-equal",
        _ => "?",
    }
}

fn read_atc_forward_message(p: &mut Per, _downlink: bool) -> Option<Value> {
    let (y, mo, d) = (
        p.constrained(1996, 2095)?,
        p.constrained(1, 12)?,
        p.constrained(1, 31)?,
    );
    let (h, mi, sec) = (
        p.constrained(0, 23)?,
        p.constrained(0, 59)?,
        p.constrained(0, 59)?,
    );
    let aircraft_id = p.ia5(2, 8)?;
    let addr = p.remaining_bytes(24)?;
    let carried_uplink = p.bit()? == 0;
    let n = p.length()?;
    let inner = p.remaining_bytes(n)?;
    let elements = atc_message_data(&inner, n, !carried_uplink);
    let mut out = json!({
        "timestamp": format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}Z"),
        "aircraft_id": aircraft_id,
        "aircraft_address": addr.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "carried_direction": if carried_uplink { "uplink" } else { "downlink" },
    });
    if let Some(els) = elements {
        out["elements"] = els;
    }
    Some(out)
}

fn protected_message(p: &mut Per, downlink: bool) -> Option<Value> {
    if p.bit()? != 0 {
        return None;
    }
    let has_algo = p.bit()? == 1;
    let has_msg = p.bit()? == 1;
    if has_algo {
        let n = p.length()?;
        p.remaining_bytes(n * 8)?;
    }
    let mut out = if !has_msg {
        json!({ "empty": true })
    } else {
        let nbits = p.length()?;
        let inner = p.remaining_bytes(nbits)?;
        atc_message(&inner, nbits, downlink)?
    };
    if let Some(nbits) = p.length()
        && let Some(ic) = p.remaining_bytes(nbits)
            && !ic.is_empty() {
                out["integrity_check"] =
                    json!(ic.iter().map(|b| format!("{b:02x}")).collect::<String>());
            }
    Some(out)
}

fn atc_message(bytes: &[u8], nbits: usize, downlink: bool) -> Option<Value> {
    let mut store = Vec::new();
    let mut p = Per::new(bytes, &mut store);
    p.bits = &p.bits[..nbits.min(p.bits.len())];

    let has_ref = p.bit()? == 1;
    let has_ack = p.bit()? == 1;
    let msg_id = p.constrained(0, 63)?;
    let msg_ref = if has_ref {
        Some(p.constrained(0, 63)?)
    } else {
        None
    };
    let (y, mo, d) = (
        p.constrained(1996, 2095)?,
        p.constrained(1, 12)?,
        p.constrained(1, 31)?,
    );
    let (h, mi, sec) = (
        p.constrained(0, 23)?,
        p.constrained(0, 59)?,
        p.constrained(0, 59)?,
    );
    let ack = if has_ack {
        if p.constrained(0, 1)? == 0 {
            "required"
        } else {
            "not-required"
        }
    } else {
        "not-required"
    };

    let (elements, route_clearances) = walk_message_data(&mut p, downlink)?;
    let mut out = json!({
        "msg_id": msg_id,
        "msg_ref": msg_ref,
        "timestamp": format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}Z"),
        "logical_ack": ack,
        "elements": elements,
    });
    if let Some(rcs) = route_clearances {
        out["route_clearances"] = rcs;
    }
    Some(out)
}

fn atc_message_data(bytes: &[u8], nbits: usize, downlink: bool) -> Option<Value> {
    let mut store = Vec::new();
    let mut p = Per::new(bytes, &mut store);
    p.bits = &p.bits[..nbits.min(p.bits.len())];
    let (elements, route_clearances) = walk_message_data(&mut p, downlink)?;
    let mut out = json!({ "elements": elements });
    if let Some(rcs) = route_clearances {
        out["route_clearances"] = rcs;
    }
    Some(out)
}

fn walk_message_data(p: &mut Per, downlink: bool) -> Option<(Value, Option<Value>)> {
    let has_constrained = p.bit()? == 1;
    let count = p.constrained(1, 5)? as usize;
    let table: &[(&str, &str, &str)] = if downlink {
        &DOWNLINK_ELEMENTS
    } else {
        &UPLINK_ELEMENTS
    };
    let idx_bits = 64 - (table.len() as u64 - 1).leading_zeros() as usize;

    let mut elements = Vec::new();
    let mut bailed = false;
    for k in 0..count {
        let idx = p.uint(idx_bits)? as usize;
        let (name, arg_ty, phrase) = table.get(idx).copied()?;
        let mut el = json!({ "element": name, "phrase": phrase });
        if arg_ty != "NULL" {
            el["argument_type"] = json!(arg_ty);
            match read_argument(p, arg_ty) {
                Some(vals) => {
                    el["text"] = json!(fill_phrase(phrase, &vals));
                    el["arguments"] = json!(vals);
                }
                None => {
                    if k + 1 < count {
                        el["note"] =
                            json!("remaining elements undecoded (argument type unsupported)");
                    }
                    bailed = true;
                    elements.push(el);
                    break;
                }
            }
        } else {
            el["text"] = json!(phrase);
        }
        elements.push(el);
    }

    let mut route_clearances = None;
    if has_constrained && !bailed
        && p.bit() == Some(0) && p.bit() == Some(1)
            && let Some(n) = p.constrained(1, 2) {
                let mut rcs = Vec::new();
                for _ in 0..n {
                    match read_route_clearance(p) {
                        Some(rc) => rcs.push(rc),
                        None => break,
                    }
                }
                if !rcs.is_empty() {
                    route_clearances = Some(json!(rcs));
                }
            }
    Some((json!(elements), route_clearances))
}

fn read_published(p: &mut Per) -> Option<String> {
    let navaid = p.bit()? == 1;
    let has_ll = p.bit()? == 1;
    let name = if navaid { p.ia5(1, 4)? } else { p.ia5(1, 5)? };
    if has_ll {
        let ll = read_latlon(p)?;
        Some(format!("{name} ({ll})"))
    } else {
        Some(name)
    }
}

fn read_distance(p: &mut Per) -> Option<String> {
    Some(if p.bit()? == 0 {
        format!("{:.1} NM", p.constrained(0, 9999)? as f64 / 10.0)
    } else {
        format!("{:.2} KM", p.constrained(0, 8000)? as f64 / 4.0)
    })
}

fn read_procedure(p: &mut Per) -> Option<String> {
    let has_transition = p.bit()? == 1;
    let ptype = match p.uint(2)? {
        0 => "ARRIVAL",
        1 => "APPROACH",
        2 => "DEPARTURE",
        _ => return None,
    };
    let name = p.ia5(1, 20)?;
    let mut s = format!("{name} ({ptype})");
    if has_transition {
        s.push_str(&format!(" TRANSITION {}", p.ia5(1, 5)?));
    }
    Some(s)
}

fn read_runway(p: &mut Per) -> Option<String> {
    let dir = p.constrained(1, 36)?;
    let cfg = match p.uint(2)? {
        0 => "L",
        1 => "R",
        2 => "C",
        _ => "",
    };
    Some(format!("RWY {dir:02}{cfg}"))
}

fn read_route_information(p: &mut Per) -> Option<String> {
    match p.uint(3)? {
        0 => read_published(p),
        1 => read_latlon(p),
        2 => {
            let a = format!("{} BRG {}", read_published(p)?, read_degrees(p)?);
            let b = format!("{} BRG {}", read_published(p)?, read_degrees(p)?);
            Some(format!("{a} / {b}"))
        }
        3 => Some(format!(
            "{} BRG {} DIST {}",
            read_published(p)?,
            read_degrees(p)?,
            read_distance(p)?
        )),
        4 => p.ia5(2, 7),
        _ => None,
    }
}

fn read_route_clearance(p: &mut Per) -> Option<Value> {
    let present: Vec<bool> = (0..9).map(|_| p.bit() == Some(1)).collect();
    let mut out = serde_json::Map::new();
    if present[0] {
        out.insert("departure_airport".into(), json!(p.ia5(4, 4)?));
    }
    if present[1] {
        out.insert("destination_airport".into(), json!(p.ia5(4, 4)?));
    }
    if present[2] {
        out.insert("departure_runway".into(), json!(read_runway(p)?));
    }
    if present[3] {
        out.insert("departure_procedure".into(), json!(read_procedure(p)?));
    }
    if present[4] {
        out.insert("arrival_runway".into(), json!(read_runway(p)?));
    }
    if present[5] {
        out.insert("approach_procedure".into(), json!(read_procedure(p)?));
    }
    if present[6] {
        out.insert("arrival_procedure".into(), json!(read_procedure(p)?));
    }
    if present[7] {
        let n = p.constrained(1, 128)? as usize;
        let mut legs = Vec::with_capacity(n);
        for _ in 0..n {
            legs.push(read_route_information(p)?);
        }
        out.insert("route".into(), json!(legs));
    }
    if present[8] {
        out.insert("additional".into(), json!("present (undecoded)"));
    }
    Some(Value::Object(out))
}

fn fill_phrase(phrase: &str, vals: &[String]) -> String {
    let mut out = String::new();
    let mut vi = 0;
    let mut rest = phrase;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => {
                if let Some(v) = vals.get(vi) {
                    out.push_str(v);
                } else {
                    out.push_str(&rest[i..i + j + 1]);
                }
                vi += 1;
                rest = &rest[i + j + 1..];
            }
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn read_argument(p: &mut Per, ty: &str) -> Option<Vec<String>> {
    Some(match ty {
        "Level" => vec![read_level(p)?],
        "LevelLevel" => vec![read_level(p)?, read_level(p)?],
        "Time" => vec![read_time(p)?],
        "TimeTime" => vec![read_time(p)?, read_time(p)?],
        "Position" => vec![read_position(p)?],
        "PositionPosition" => vec![read_position(p)?, read_position(p)?],
        "Speed" => vec![read_speed(p)?],
        "SpeedSpeed" => vec![read_speed(p)?, read_speed(p)?],
        "Degrees" => vec![read_degrees(p)?],
        "Airport" => vec![p.ia5(4, 4)?],
        "LevelPosition" => vec![read_level(p)?, read_position(p)?],
        "LevelTime" => vec![read_level(p)?, read_time(p)?],
        "LevelSpeed" => vec![read_level(p)?, read_speed(p)?, read_speed(p)?],
        "PositionLevel" => vec![read_position(p)?, read_level(p)?],
        "PositionTime" => vec![read_position(p)?, read_time(p)?],
        "PositionSpeed" => vec![read_position(p)?, read_speed(p)?],
        "PositionDegrees" => vec![read_position(p)?, read_degrees(p)?],
        "TimeLevel" => vec![read_time(p)?, read_level(p)?],
        "TimePosition" => vec![read_time(p)?, read_position(p)?],
        "DirectionDegrees" => vec![read_direction(p)?, read_degrees(p)?],
        "RouteClearanceIndex" => {
            vec![format!("(route clearance #{})", p.constrained(1, 2)?)]
        }
        "PositionRouteClearanceIndex" => vec![
            read_position(p)?,
            format!("(route clearance #{})", p.constrained(1, 2)?),
        ],

        "Frequency" => vec![read_frequency(p)?],
        "UnitNameFrequency" => vec![read_unit_name(p)?, read_frequency(p)?],
        "PositionUnitNameFrequency" => {
            vec![read_position(p)?, read_unit_name(p)?, read_frequency(p)?]
        }
        "TimeUnitNameFrequency" => {
            vec![read_time(p)?, read_unit_name(p)?, read_frequency(p)?]
        }

        "Altimeter" => vec![read_altimeter(p)?],
        "FacilityDesignation" => vec![read_facility_designation(p)?],
        "Facility" => vec![read_facility(p)?],
        "FacilityDesignationAltimeter" => {
            vec![read_facility_designation(p)?, read_altimeter(p)?]
        }
        "FacilityDesignationATISCode" => {
            vec![read_facility_designation(p)?, read_atis_code(p)?]
        }

        "ATISCode" => vec![read_atis_code(p)?],
        "Code" => vec![read_code(p)?],
        "FreeText" => vec![read_free_text(p)?],
        "VersionNumber" => vec![format!("v{}", p.constrained(0, 15)?)],
        "TrafficType" => vec![read_traffic_type(p)?],
        "ClearanceType" => vec![read_clearance_type(p)?],
        "ErrorInformation" => vec![read_error_information(p)?],

        "ProcedureName" => vec![read_procedure(p)?],
        "PositionProcedureName" => vec![read_position(p)?, read_procedure(p)?],
        "RunwayRVR" => vec![read_runway(p)?, read_rvr(p)?],

        "SpeedTypeSpeedTypeSpeedType" => read_speed_type_triple(p)?,
        "SpeedTypeSpeedTypeSpeedTypeSpeed" => {
            let mut v = read_speed_type_triple(p)?;
            v.push(read_speed(p)?);
            v
        }
        "LevelSpeedSpeed" => vec![read_level(p)?, read_speed(p)?, read_speed(p)?],
        "PositionSpeedSpeed" => {
            vec![read_position(p)?, read_speed(p)?, read_speed(p)?]
        }
        "TimeSpeed" => vec![read_time(p)?, read_speed(p)?],
        "SpeedTime" => vec![read_speed(p)?, read_time(p)?],
        "TimeSpeedSpeed" => vec![read_time(p)?, read_speed(p)?, read_speed(p)?],
        "PositionLevelLevel" => {
            vec![read_position(p)?, read_level(p)?, read_level(p)?]
        }
        "PositionLevelSpeed" => {
            vec![read_position(p)?, read_level(p)?, read_speed(p)?]
        }
        "PositionTimeTime" => {
            vec![read_position(p)?, read_time(p)?, read_time(p)?]
        }
        "PositionTimeLevel" => {
            vec![read_position(p)?, read_time(p)?, read_level(p)?]
        }
        "TimePositionLevel" => {
            vec![read_time(p)?, read_position(p)?, read_level(p)?]
        }
        "TimePositionLevelSpeed" => {
            vec![
                read_time(p)?,
                read_position(p)?,
                read_level(p)?,
                read_speed(p)?,
                read_speed(p)?,
            ]
        }

        "DistanceSpecifiedDirection" => {
            let (d, dir) = read_distance_specified_direction(p)?;
            vec![d, dir]
        }
        "PositionDistanceSpecifiedDirection" => {
            let pos = read_position(p)?;
            let (d, dir) = read_distance_specified_direction(p)?;
            vec![pos, d, dir]
        }
        "TimeDistanceSpecifiedDirection" => {
            let t = read_time(p)?;
            let (d, dir) = read_distance_specified_direction(p)?;
            vec![t, d, dir]
        }
        "DistanceSpecifiedDirectionTime" => {
            let (d, dir) = read_distance_specified_direction(p)?;
            vec![d, dir, read_time(p)?]
        }

        "ToFromPosition" => vec![read_tofrom(p)?, read_position(p)?],
        "TimeToFromPosition" => {
            vec![read_time(p)?, read_tofrom(p)?, read_position(p)?]
        }
        "TimeDistanceToFromPosition" => vec![
            read_time(p)?,
            read_distance(p)?,
            read_tofrom(p)?,
            read_position(p)?,
        ],

        "VerticalRate" => vec![read_vertical_rate(p)?],
        "RemainingFuelPersonsOnBoard" => {
            vec![read_time(p)?, format!("{}", p.constrained(1, 1024)?)]
        }

        "HoldClearance" => read_hold_clearance(p)?,
        "DepartureClearance" => read_departure_clearance(p)?,
        "PositionReport" => read_position_report(p)?,

        _ => return None,
    })
}

fn read_level(p: &mut Per) -> Option<String> {
    if p.bit()? == 0 {
        read_level_type(p)
    } else {
        Some(format!(
            "{} TO {}",
            read_level_type(p)?,
            read_level_type(p)?
        ))
    }
}

fn read_level_type(p: &mut Per) -> Option<String> {
    Some(match p.uint(2)? {
        0 => format!("{} FT", p.constrained(-60, 7000)? * 10),
        1 => format!("{} M", p.constrained(-30, 25_000)?),
        2 => format!("FL{}", p.constrained(30, 700)?),
        _ => format!("{} M", p.constrained(100, 2500)? * 10),
    })
}

fn read_time(p: &mut Per) -> Option<String> {
    Some(format!(
        "{:02}:{:02}",
        p.constrained(0, 23)?,
        p.constrained(0, 59)?
    ))
}

fn read_speed(p: &mut Per) -> Option<String> {
    Some(match p.uint(3)? {
        0 => format!("{} KT IAS", p.constrained(0, 400)?),
        1 => format!("{} KM/H IAS", p.constrained(0, 800)?),
        2 => format!("{} KT TAS", p.constrained(0, 2000)?),
        3 => format!("{} KM/H TAS", p.constrained(0, 4000)?),
        4 => format!("{} KT GS", p.constrained(-50, 2000)?),
        5 => format!("{} KM/H GS", p.constrained(-100, 4000)?),
        6 => format!("M{:.3}", p.constrained(500, 4000)? as f64 / 1000.0),
        _ => return None,
    })
}

fn read_degrees(p: &mut Per) -> Option<String> {
    let mag = p.bit()? == 0;
    Some(format!(
        "{}°{}",
        p.constrained(1, 360)?,
        if mag { "M" } else { "T" }
    ))
}

fn read_direction(p: &mut Per) -> Option<String> {
    const DIRS: [&str; 11] = [
        "LEFT",
        "RIGHT",
        "EITHER SIDE",
        "NORTH",
        "SOUTH",
        "EAST",
        "WEST",
        "NORTH-EAST",
        "NORTH-WEST",
        "SOUTH-EAST",
        "SOUTH-WEST",
    ];
    DIRS.get(p.constrained(0, 10)? as usize)
        .map(|s| s.to_string())
}

fn read_position(p: &mut Per) -> Option<String> {
    match p.uint(3)? {
        0 => {
            let has_ll = p.bit()? == 1;
            let name = p.ia5(1, 5)?;
            if has_ll {
                let ll = read_latlon(p)?;
                Some(format!("{name} ({ll})"))
            } else {
                Some(name)
            }
        }
        1 => {
            let has_ll = p.bit()? == 1;
            let name = p.ia5(1, 4)?;
            if has_ll {
                let ll = read_latlon(p)?;
                Some(format!("{name} ({ll})"))
            } else {
                Some(name)
            }
        }
        2 => p.ia5(4, 4),
        3 => read_latlon(p),
        _ => None,
    }
}

fn read_latlon(p: &mut Per) -> Option<String> {
    let has_lat = p.bit()? == 1;
    let has_lon = p.bit()? == 1;
    let mut parts = Vec::new();
    if has_lat {
        let v = read_lat_or_lon(p, 90_000, 89)?;
        let dir = if p.bit()? == 0 { "N" } else { "S" };
        parts.push(format!("{v}{dir}"));
    }
    if has_lon {
        let v = read_lat_or_lon(p, 180_000, 179)?;
        let dir = if p.bit()? == 0 { "E" } else { "W" };
        parts.push(format!("{v}{dir}"));
    }
    Some(parts.join(" "))
}

fn read_lat_or_lon(p: &mut Per, max_milli: i64, max_whole: i64) -> Option<String> {
    Some(match p.uint(2)? {
        0 => format!("{:.3}°", p.constrained(0, max_milli)? as f64 / 1000.0),
        1 => {
            let d = p.constrained(0, max_whole)?;
            let m = p.constrained(0, 5999)? as f64 / 100.0;
            format!("{d}°{m:.2}'")
        }
        2 => {
            let d = p.constrained(0, max_whole)?;
            let m = p.constrained(0, 59)?;
            let s = p.constrained(0, 59)?;
            format!("{d}°{m}'{s}\"")
        }
        _ => return None,
    })
}

fn read_frequency(p: &mut Per) -> Option<String> {
    Some(match p.uint(2)? {
        0 => format!("{} kHz", p.constrained(2850, 28000)?),
        1 => format!("{:.3} MHz", p.constrained(23600, 27398)? as f64 * 0.005),
        2 => format!("{:.3} MHz", p.constrained(9000, 15999)? as f64 * 0.025),
        _ => {
            let mut s = String::with_capacity(12);
            for _ in 0..12 {
                s.push((b'0' + p.uint(4)? as u8) as char);
            }
            format!("SAT {s}")
        }
    })
}

fn read_altimeter(p: &mut Per) -> Option<String> {
    Some(if p.bit()? == 0 {
        format!("{:.2} inHg", p.constrained(2200, 3200)? as f64 / 100.0)
    } else {
        format!("{:.1} hPa", p.constrained(7500, 12500)? as f64 / 10.0)
    })
}

fn read_atis_code(p: &mut Per) -> Option<String> {
    Some((p.uint(7)? as u8 as char).to_string())
}

fn read_facility_designation(p: &mut Per) -> Option<String> {
    p.ia5(4, 8)
}

fn read_facility(p: &mut Per) -> Option<String> {
    if p.bit()? == 0 {
        Some("(none)".to_string())
    } else {
        read_facility_designation(p)
    }
}

fn read_code(p: &mut Per) -> Option<String> {
    let mut s = String::with_capacity(4);
    for _ in 0..4 {
        s.push((b'0' + p.constrained(0, 7)? as u8) as char);
    }
    Some(s)
}

fn read_free_text(p: &mut Per) -> Option<String> {
    p.ia5(1, 256)
}

fn read_traffic_type(p: &mut Per) -> Option<String> {
    if p.bit()? != 0 {
        return Some("(extended)".to_string());
    }
    const TT: [&str; 6] = [
        "NONE",
        "OPPOSITE DIRECTION",
        "SAME DIRECTION",
        "CONVERGING",
        "CROSSING",
        "DIVERGING",
    ];
    TT.get(p.constrained(0, 5)? as usize).map(|s| s.to_string())
}

fn read_clearance_type(p: &mut Per) -> Option<String> {
    if p.bit()? != 0 {
        return Some("(extended)".to_string());
    }
    const CT: [&str; 12] = [
        "NONE",
        "APPROACH",
        "DEPARTURE",
        "FURTHER",
        "START-UP",
        "PUSHBACK",
        "TAXI",
        "TAKE-OFF",
        "LANDING",
        "OCEANIC",
        "EN-ROUTE",
        "DOWNSTREAM",
    ];
    CT.get(p.constrained(0, 11)? as usize)
        .map(|s| s.to_string())
}

fn read_error_information(p: &mut Per) -> Option<String> {
    if p.bit()? != 0 {
        return Some("(extended)".to_string());
    }
    const EI: [&str; 5] = [
        "UNRECOGNIZED MSG REFERENCE NUMBER",
        "LOGICAL ACKNOWLEDGMENT NOT ACCEPTED",
        "INSUFFICIENT RESOURCES",
        "INVALID MESSAGE ELEMENT COMBINATION",
        "INVALID MESSAGE ELEMENT",
    ];
    EI.get(p.constrained(0, 4)? as usize).map(|s| s.to_string())
}

fn read_tofrom(p: &mut Per) -> Option<String> {
    Some(
        if p.constrained(0, 1)? == 0 {
            "TO"
        } else {
            "FROM"
        }
        .to_string(),
    )
}

fn read_unit_name(p: &mut Per) -> Option<String> {
    let has_name = p.bit()? == 1;
    let designation = read_facility_designation(p)?;
    let name = if has_name { Some(p.ia5(3, 18)?) } else { None };
    let function = read_facility_function(p)?;
    Some(match name {
        Some(n) => format!("{designation} {n} {function}"),
        None => format!("{designation} {function}"),
    })
}

fn read_facility_function(p: &mut Per) -> Option<String> {
    if p.bit()? != 0 {
        return Some("(extended)".to_string());
    }
    const FF: [&str; 9] = [
        "CENTER",
        "APPROACH",
        "TOWER",
        "FINAL",
        "GROUND",
        "DELIVERY",
        "DEPARTURE",
        "CONTROL",
        "RADIO",
    ];
    FF.get(p.constrained(0, 8)? as usize).map(|s| s.to_string())
}

fn read_rvr(p: &mut Per) -> Option<String> {
    Some(if p.bit()? == 0 {
        format!("{} FT", p.constrained(0, 6100)?)
    } else {
        format!("{} M", p.constrained(0, 1500)?)
    })
}

fn read_vertical_rate(p: &mut Per) -> Option<String> {
    Some(if p.bit()? == 0 {
        format!("{} FPM", p.constrained(0, 3000)? * 10)
    } else {
        format!("{} M/MIN", p.constrained(0, 1000)? * 10)
    })
}

fn read_speed_type(p: &mut Per) -> Option<String> {
    if p.bit()? != 0 {
        return Some("(extended)".to_string());
    }
    const ST: [&str; 9] = [
        "NONE",
        "INDICATED",
        "TRUE",
        "GROUND",
        "MACH",
        "APPROACH",
        "CRUISE",
        "MINIMUM",
        "MAXIMUM",
    ];
    ST.get(p.constrained(0, 8)? as usize).map(|s| s.to_string())
}

fn read_speed_type_triple(p: &mut Per) -> Option<Vec<String>> {
    Some(vec![
        read_speed_type(p)?,
        read_speed_type(p)?,
        read_speed_type(p)?,
    ])
}

fn read_distance_specified_direction(p: &mut Per) -> Option<(String, String)> {
    let dist = if p.bit()? == 0 {
        format!("{} NM", p.constrained(1, 250)?)
    } else {
        format!("{} KM", p.constrained(1, 500)?)
    };
    Some((dist, read_direction(p)?))
}

fn read_hold_clearance(p: &mut Per) -> Option<Vec<String>> {
    let has_leg = p.bit()? == 1;
    let position = read_position(p)?;
    let level = read_level(p)?;
    let degrees = read_degrees(p)?;
    let direction = read_direction(p)?;
    let leg = if has_leg {
        read_leg_type(p)?
    } else {
        "(none)".to_string()
    };
    Some(vec![position, level, degrees, direction, leg])
}

fn read_leg_type(p: &mut Per) -> Option<String> {
    Some(if p.bit()? == 0 {
        if p.bit()? == 0 {
            format!("{} NM", p.constrained(0, 50)?)
        } else {
            format!("{} KM", p.constrained(1, 128)?)
        }
    } else {
        format!("{} MIN", p.constrained(0, 10)?)
    })
}

fn read_departure_clearance(p: &mut Per) -> Option<Vec<String>> {
    let has_flight_info = p.bit()? == 1;
    let has_further = p.bit()? == 1;
    let flight_id = p.ia5(2, 8)?;
    let limit = read_position(p)?;
    let mut s = format!("{flight_id} CLEARED TO {limit}");
    if has_flight_info || has_further {
        s.push_str(" (+flight-info/further-instructions present, undecoded)");
    }
    Some(vec![s])
}

fn read_position_report(p: &mut Per) -> Option<Vec<String>> {
    let mut any_optional = false;
    for _ in 0..19 {
        if p.bit()? == 1 {
            any_optional = true;
        }
    }
    let position = read_position(p)?;
    let time = read_time(p)?;
    let level = read_level(p)?;
    let mut s = format!("{position} {time} {level}");
    if any_optional {
        s.push_str(" (+optional fields present, undecoded)");
        return None;
    }
    Some(vec![s])
}

fn read_short_tsap(p: &mut Per) -> Option<Value> {
    let has_ars = p.bit()? == 1;
    let mut out = serde_json::Map::new();
    if has_ars {
        let ars = p.remaining_bytes(3 * 8)?;
        out.insert(
            "ars".into(),
            json!(ars.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        );
    }
    let n = p.constrained(10, 11)? as usize;
    let sel = p.remaining_bytes(n * 8)?;
    out.insert(
        "loc_sys_nsel_tsel".into(),
        json!(sel.iter().map(|b| format!("{b:02x}")).collect::<String>()),
    );
    Some(Value::Object(out))
}

fn read_long_tsap(p: &mut Per) -> Option<Value> {
    let rdp = p.remaining_bytes(5 * 8)?;
    Some(json!({
        "rdp": rdp.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "short_tsap": read_short_tsap(p)?,
    }))
}

fn read_ap_address(p: &mut Per) -> Option<Value> {
    Some(if p.bit()? == 0 {
        json!({ "long_tsap": read_long_tsap(p)? })
    } else {
        json!({ "short_tsap": read_short_tsap(p)? })
    })
}

fn read_ae_qualifier_version(p: &mut Per, with_address: bool) -> Option<Value> {
    let ae = p.constrained(0, 255)?;
    let ver = p.constrained(1, 255)?;
    let mut out = json!({ "ae_qualifier": ae, "ap_version": ver });
    if with_address {
        out["ap_address"] = read_ap_address(p)?;
    }
    Some(out)
}

fn read_app_list(p: &mut Per, with_address: bool) -> Option<Vec<Value>> {
    let n = p.constrained(1, 256)? as usize;
    let mut out = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        out.push(read_ae_qualifier_version(p, with_address)?);
    }
    Some(out)
}

fn read_cm_datetime(p: &mut Per) -> Option<String> {
    let (y, mo, d) = (
        p.constrained(1996, 2095)?,
        p.constrained(1, 12)?,
        p.constrained(1, 31)?,
    );
    let (h, mi) = (p.constrained(0, 23)?, p.constrained(0, 59)?);
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}Z"))
}

fn cm_abort_reason(r: i64) -> Option<&'static str> {
    const R: [&str; 10] = [
        "timer-expired",
        "undefined-error",
        "invalid-PDU",
        "protocol-error",
        "dialogue-acceptance-not-permitted",
        "dialogue-end-not-accepted",
        "communication-service-error",
        "communication-service-failure",
        "invalid-QOS-parameter",
        "expected-PDU-missing",
    ];
    R.get(r as usize).copied()
}

fn read_cm_abort(p: &mut Per, out: &mut Value) -> Option<()> {
    if p.bit()? == 0 {
        let r = p.constrained(0, 9)?;
        out["reason"] = json!(r);
        if let Some(t) = cm_abort_reason(r) {
            out["reason_text"] = json!(t);
        }
    } else {
        out["reason"] = json!("(extended)");
    }
    Some(())
}

fn read_cm_logon_response(p: &mut Per, out: &mut Value) -> Option<()> {
    let has_air = p.bit()? == 1;
    let has_ground = p.bit()? == 1;
    if has_air {
        out["air_initiated_applications"] = json!(read_app_list(p, true)?);
    }
    if has_ground {
        out["ground_only_initiated_applications"] = json!(read_app_list(p, false)?);
    }
    Some(())
}

fn read_cm_logon_request(p: &mut Per) -> Option<Value> {
    let has_ground = p.bit()? == 1;
    let has_air_only = p.bit()? == 1;
    let has_facility = p.bit()? == 1;
    let has_dep = p.bit()? == 1;
    let has_dest = p.bit()? == 1;
    let has_etd = p.bit()? == 1;
    let flight_id = p.ia5(2, 8)?;
    let long_tsap = read_long_tsap(p)?;
    let mut out = json!({
        "flight_id": flight_id,
        "cm_long_tsap": long_tsap,
    });
    if has_ground {
        out["ground_initiated_applications"] = json!(read_app_list(p, true)?);
    }
    if has_air_only {
        out["air_only_initiated_applications"] = json!(read_app_list(p, false)?);
    }
    if has_facility {
        out["facility_designation"] = json!(read_facility_designation(p)?);
    }
    if has_dep {
        out["airport_departure"] = json!(p.ia5(4, 4)?);
    }
    if has_dest {
        out["airport_destination"] = json!(p.ia5(4, 4)?);
    }
    if has_etd {
        out["etd"] = json!(read_cm_datetime(p)?);
    }
    Some(out)
}

pub fn parse_cm_ground(bytes: &[u8]) -> Option<Value> {
    let mut store = Vec::new();
    let mut p = Per::new(bytes, &mut store);
    if p.bit()? != 0 {
        return None;
    }
    let kind = match p.uint(3)? {
        0 => "logon-response",
        1 => "update",
        2 => "contact-request",
        3 => "forward-request",
        4 => "abort",
        5 => "forward-response",
        _ => return None,
    };
    let mut out = json!({ "application": "CM", "pdu": kind });
    match kind {
        "logon-response" | "update" => {
            read_cm_logon_response(&mut p, &mut out)?;
        }
        "contact-request" => {
            out["facility_designation"] = json!(read_facility_designation(&mut p)?);
            out["address"] = read_long_tsap(&mut p)?;
        }
        "forward-request" => {
            out["request"] = read_cm_logon_request(&mut p)?;
        }
        "abort" => {
            read_cm_abort(&mut p, &mut out)?;
        }
        "forward-response" => {
            out["response"] = json!(match p.constrained(0, 2)? {
                0 => "success",
                1 => "incompatible-version",
                2 => "service-not-supported",
                _ => "?",
            });
        }
        _ => {}
    }
    Some(out)
}

pub fn parse_cm_logon(bytes: &[u8]) -> Option<Value> {
    let mut store = Vec::new();
    let mut p = Per::new(bytes, &mut store);
    if p.bit()? != 0 {
        return None;
    }
    match p.uint(2)? {
        0 => {
            let req = read_cm_logon_request(&mut p)?;
            let mut out = json!({ "application": "CM", "pdu": "logon-request" });
            if let Value::Object(m) = req {
                for (k, v) in m {
                    out[k] = v;
                }
            }
            Some(out)
        }
        1 => {
            let r = p.constrained(0, 1)?;
            Some(json!({
                "application": "CM",
                "pdu": "contact-response",
                "response": if r == 0 { "contactSuccess" } else { "contactNotSuccessful" },
            }))
        }
        2 => {
            let mut out = json!({ "application": "CM", "pdu": "abort" });
            read_cm_abort(&mut p, &mut out)?;
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cm_ground_logon_response() {
        let v = parse_cm_ground(&[0b0000_0000]).unwrap();
        assert_eq!(v["application"], "CM");
        assert_eq!(v["pdu"], "logon-response");
        assert!(v.get("air_initiated_applications").is_none());
        assert!(v.get("ground_only_initiated_applications").is_none());
    }

    struct Bits(Vec<u8>);
    impl Bits {
        fn new() -> Self {
            Bits(Vec::new())
        }
        fn push(&mut self, v: u64, n: usize) {
            for k in (0..n).rev() {
                self.0.push(((v >> k) & 1) as u8);
            }
        }
        fn ia5(&mut self, s: &str) {
            for c in s.bytes() {
                self.push(c as u64, 7);
            }
        }
        fn bytes(&self) -> Vec<u8> {
            self.0
                .chunks(8)
                .map(|c| {
                    c.iter()
                        .enumerate()
                        .fold(0u8, |v, (i, &b)| v | (b << (7 - i)))
                })
                .collect()
        }
    }

    fn build_downlink_wilco() -> Vec<u8> {
        let mut m = Bits::new();
        m.push(0, 1);
        m.push(0, 1);
        m.push(12, 6);
        m.push((2026 - 1996) as u64, 7);
        m.push(6 - 1, 4);
        m.push(11 - 1, 5);
        m.push(1, 5);
        m.push(22, 6);
        m.push(33, 6);
        m.push(0, 1);
        m.push(0, 3);
        m.push(0, 7);
        let inner_bits = m.0.len();

        let mut o = Bits::new();
        o.push(0, 1);
        o.push(3, 2);
        o.push(0, 1);
        o.push(0, 1);
        o.push(1, 1);
        o.push(0, 1);
        o.push(inner_bits as u64, 7);
        o.0.extend(&m.0);
        o.push(0, 1);
        o.push(0, 7);
        o.bytes()
    }

    #[test]
    fn downlink_wilco_decodes() {
        let v = parse_apdu(&build_downlink_wilco()).expect("apdu");
        assert_eq!(v["application"], "CPDLC");
        assert_eq!(v["direction"], "downlink");
        assert_eq!(v["pdu"], "send");
        let msg = &v["message"];
        assert_eq!(msg["msg_id"], 12);
        assert_eq!(msg["timestamp"], "2026-06-11T01:22:33Z");
        assert_eq!(msg["elements"][0]["element"], "dM0NULL");
        assert_eq!(msg["elements"][0]["phrase"], "WILCO");
    }

    #[test]
    fn uplink_element_with_argument_reports_type() {
        let mut m = Bits::new();
        m.push(0, 1);
        m.push(0, 1);
        m.push(5, 6);
        m.push(30, 7);
        m.push(0, 4);
        m.push(0, 5);
        m.push(10, 5);
        m.push(0, 6);
        m.push(0, 6);
        m.push(0, 1);
        m.push(0, 3);
        m.push(20, 8);
        m.push(0, 1);
        m.push(2, 2);
        m.push(360 - 30, 10);
        let inner_bits = m.0.len();
        let mut o = Bits::new();
        o.push(0, 1);
        o.push(3, 3);
        o.push(0, 1);
        o.push(0, 1);
        o.push(1, 1);
        o.push(0, 1);
        o.push(inner_bits as u64, 7);
        o.0.extend(&m.0);
        o.push(0, 1);
        o.push(0, 7);
        let v = parse_pdus(&o.bytes(), false).expect("apdu");
        let el = &v["message"]["elements"][0];
        assert_eq!(el["element"], "uM20Level");
        assert_eq!(el["phrase"], "CLIMB TO [level]");
        assert_eq!(el["argument_type"], "Level");
        assert_eq!(el["text"], "CLIMB TO FL360");
    }

    fn push_short_tsap(b: &mut Bits, ars: Option<&[u8]>, sel: &[u8]) {
        match ars {
            Some(a) => {
                b.push(1, 1);
                for &x in a {
                    b.push(x as u64, 8);
                }
            }
            None => b.push(0, 1),
        }
        b.push((sel.len() - 10) as u64, 1);
        for &x in sel {
            b.push(x as u64, 8);
        }
    }

    fn push_long_tsap(b: &mut Bits, rdp: &[u8], ars: Option<&[u8]>, sel: &[u8]) {
        for &x in rdp {
            b.push(x as u64, 8);
        }
        push_short_tsap(b, ars, sel);
    }

    #[test]
    fn cm_logon_request_flight_id_decodes() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(0, 2);
        b.push(0, 6);
        b.push(4, 3);
        for c in b"UAL123" {
            b.push(*c as u64, 7);
        }
        let sel: Vec<u8> = (0..10).collect();
        push_long_tsap(
            &mut b,
            &[0x47, 0x00, 0x27, 0x01, 0x02],
            Some(&[0xAB, 0xCD, 0xEF]),
            &sel,
        );
        let v = parse_cm_logon(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "logon-request");
        assert_eq!(v["flight_id"], "UAL123");
        assert_eq!(v["cm_long_tsap"]["rdp"], "4700270102");
        assert_eq!(v["cm_long_tsap"]["short_tsap"]["ars"], "abcdef");
        assert_eq!(
            v["cm_long_tsap"]["short_tsap"]["loc_sys_nsel_tsel"],
            "00010203040506070809"
        );
    }

    #[test]
    fn cm_contact_response_and_abort() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(1, 2);
        b.push(1, 1);
        let v = parse_cm_logon(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "contact-response");
        assert_eq!(v["response"], "contactNotSuccessful");
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(2, 2);
        b.push(0, 1);
        b.push(3, 4);
        let v = parse_cm_logon(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "abort");
        assert_eq!(v["reason"], 3);
        assert_eq!(v["reason_text"], "protocol-error");
    }

    #[test]
    fn cm_ground_contact_request_full() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(2, 3);
        b.push(4 - 4, 3);
        for c in b"KZAK" {
            b.push(*c as u64, 7);
        }
        let sel: Vec<u8> = (20..31).collect();
        push_long_tsap(&mut b, &[0x10, 0x20, 0x30, 0x40, 0x50], None, &sel);
        let v = parse_cm_ground(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "contact-request");
        assert_eq!(v["facility_designation"], "KZAK");
        assert_eq!(v["address"]["rdp"], "1020304050");
        assert!(v["address"]["short_tsap"].get("ars").is_none());
        assert_eq!(
            v["address"]["short_tsap"]["loc_sys_nsel_tsel"]
                .as_str()
                .unwrap()
                .len(),
            22
        );
    }

    #[test]
    fn cm_ground_forward_response_enum() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(5, 3);
        b.push(1, 2);
        let v = parse_cm_ground(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "forward-response");
        assert_eq!(v["response"], "incompatible-version");
    }

    #[test]
    fn cm_ground_logon_response_with_app_lists() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(0, 3);
        b.push(1, 1);
        b.push(0, 1);
        b.push(1 - 1, 8);
        b.push(22, 8);
        b.push(1 - 1, 8);
        b.push(1, 1);
        let sel: Vec<u8> = (0..10).collect();
        push_short_tsap(&mut b, None, &sel);
        let v = parse_cm_ground(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "logon-response");
        let apps = v["air_initiated_applications"].as_array().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0]["ae_qualifier"], 22);
        assert_eq!(apps[0]["ap_version"], 1);
        assert!(apps[0]["ap_address"]["short_tsap"].is_object());
        assert!(v.get("ground_only_initiated_applications").is_none());
    }

    #[test]
    fn cm_ground_abort_reason() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(4, 3);
        b.push(0, 1);
        b.push(5, 4);
        let v = parse_cm_ground(&b.bytes()).expect("cm");
        assert_eq!(v["pdu"], "abort");
        assert_eq!(v["reason_text"], "dialogue-end-not-accepted");
    }

    #[test]
    fn protected_user_abort_reason_uses_four_bits() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(0, 2);
        b.push(0, 1);
        b.push(12, 4);
        let v = parse_pdus(&b.bytes(), true).expect("apdu");
        assert_eq!(v["pdu"], "abort-user");
        assert_eq!(v["reason"], 12);
        assert_eq!(v["reason_text"], "invalid-CPDLC-message");
    }

    #[test]
    fn provider_abort_reason_named() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(1, 3);
        b.push(0, 1);
        b.push(4, 3);
        let v = parse_pdus(&b.bytes(), false).expect("apdu");
        assert_eq!(v["pdu"], "abort-provider");
        assert_eq!(v["reason_text"], "communication-service-error");
    }

    #[test]
    fn choice_extension_alternative_reported() {
        let mut b = Bits::new();
        b.push(1, 1);
        b.push(0, 1);
        b.push(0, 6);
        let v = parse_pdus(&b.bytes(), true).expect("apdu");
        assert_eq!(v["pdu"], "extension-alternative");
        assert_eq!(v["extension_index"], 0);
    }

    #[test]
    fn integrity_check_is_consumed_and_reported() {
        let mut m = Bits::new();
        m.push(0, 1);
        m.push(0, 1);
        m.push(9, 6);
        m.push(30, 7);
        m.push(5, 4);
        m.push(10, 5);
        m.push(0, 5);
        m.push(0, 6);
        m.push(0, 6);
        m.push(0, 1);
        m.push(0, 3);
        m.push(0, 7);
        let inner_bits = m.0.len();
        let mut o = Bits::new();
        o.push(0, 1);
        o.push(3, 2);
        o.push(0, 1);
        o.push(0, 1);
        o.push(1, 1);
        o.push(0, 1);
        o.push(inner_bits as u64, 7);
        o.0.extend(&m.0);
        o.push(0, 1);
        o.push(16, 7);
        o.push(0xBEEF, 16);
        let v = parse_apdu(&o.bytes()).expect("apdu");
        assert_eq!(v["pdu"], "send");
        assert_eq!(v["message"]["integrity_check"], "beef");
    }

    #[test]
    fn fragmented_length_determinant_chains() {
        let mut b = Bits::new();
        b.push(0b11, 2);
        b.push(1, 6);
        b.push(0, 1);
        b.push(3, 7);
        let bytes = b.bytes();
        let mut store = Vec::new();
        let mut p = Per::new(&bytes, &mut store);
        assert_eq!(p.length(), Some(16384 + 3));
    }

    #[test]
    fn forward_response_enum_decodes() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(5, 3);
        b.push(0, 1);
        b.push(2, 2);
        let v = parse_pdus(&b.bytes(), false).expect("apdu");
        assert_eq!(v["pdu"], "forward-response");
        assert_eq!(v["response"], "version-not-equal");
    }

    #[test]
    fn forward_message_header_and_carried_uplink_decodes() {
        let mut data = Bits::new();
        data.push(0, 1);
        data.push(0, 3);
        data.push(0, 8);
        let data_bits = data.0.len();

        let mut b = Bits::new();
        b.push(0, 1);
        b.push(4, 3);
        b.push(30, 7);
        b.push(5, 4);
        b.push(10, 5);
        b.push(8, 5);
        b.push(15, 6);
        b.push(0, 6);
        b.push(4 - 2, 3);
        b.ia5("DLH7");
        b.push(0xAB, 8);
        b.push(0xCD, 8);
        b.push(0xEF, 8);
        b.push(0, 1);
        b.push(0, 1);
        b.push(data_bits as u64, 7);
        b.0.extend(&data.0);

        let v = parse_pdus(&b.bytes(), false).expect("apdu");
        assert_eq!(v["pdu"], "forward");
        let f = &v["forward"];
        assert_eq!(f["timestamp"], "2026-06-11T08:15:00Z");
        assert_eq!(f["aircraft_id"], "DLH7");
        assert_eq!(f["aircraft_address"], "abcdef");
        assert_eq!(f["carried_direction"], "uplink");
        assert_eq!(f["elements"]["elements"][0]["element"], "uM0NULL");
        assert_eq!(f["elements"]["elements"][0]["text"], "UNABLE");
    }

    #[test]
    fn acse_rlrq_release_reason_decodes() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(2, 3);
        b.push(0, 1);
        b.push(1, 1);
        b.push(0, 1);
        b.push(30, 5);
        let v = parse_acse_apdu(&b.bytes()).expect("acse");
        assert_eq!(v["application"], "ACSE");
        assert_eq!(v["pdu"], "rlrq");
        assert_eq!(v["reason"], 30);
        assert_eq!(v["reason_text"], "user-defined");
    }

    #[test]
    fn acse_rlre_not_finished_reason() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(3, 3);
        b.push(0, 1);
        b.push(1, 1);
        b.push(0, 1);
        b.push(1, 5);
        let v = parse_acse_apdu(&b.bytes()).expect("acse");
        assert_eq!(v["pdu"], "rlre");
        assert_eq!(v["reason_text"], "not-finished");
    }

    #[test]
    fn acse_abrt_source_and_diagnostic() {
        let mut b = Bits::new();
        b.push(0, 1);
        b.push(4, 3);
        b.push(0, 1);
        b.push(1, 1);
        b.push(0, 1);
        b.push(1, 1);
        b.push(0, 1);
        b.push(1, 3);
        let v = parse_acse_apdu(&b.bytes()).expect("acse");
        assert_eq!(v["pdu"], "abrt");
        assert_eq!(v["abort_source"], "acse-service-provider");
        assert_eq!(v["abort_diagnostic"], "protocol-error");
    }

    #[test]
    fn acse_aarq_aare_recognised_and_deferred() {
        for (idx, kind) in [(0u64, "aarq"), (1, "aare")] {
            let mut b = Bits::new();
            b.push(0, 1);
            b.push(idx, 3);
            let v = parse_acse_apdu(&b.bytes()).expect("acse");
            assert_eq!(v["pdu"], kind);
            assert!(v["note"].as_str().unwrap().contains("deferred"));
        }
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;

    #[test]
    fn cleared_route_decodes() {
        struct B(Vec<u8>);
        impl B {
            fn push(&mut self, v: u64, n: usize) {
                for k in (0..n).rev() {
                    self.0.push(((v >> k) & 1) as u8);
                }
            }
            fn ia5(&mut self, s: &str) {
                for c in s.bytes() {
                    self.push(c as u64, 7);
                }
            }
        }
        let mut m = B(Vec::new());
        m.push(0, 1);
        m.push(0, 1);
        m.push(3, 6);
        m.push(30, 7);
        m.push(5, 4);
        m.push(10, 5);
        m.push(2, 5);
        m.push(0, 6);
        m.push(0, 6);
        m.push(1, 1);
        m.push(0, 3);
        m.push(80, 8);
        m.push(0, 1);
        m.push(0, 1);
        m.push(1, 1);
        m.push(0, 1);
        m.push(0b010000010, 9);
        m.ia5("KSFO");
        m.push(1, 7);
        m.push(4, 3);
        m.push(2, 3);
        m.ia5("J501");
        m.push(0, 3);
        m.push(0, 1);
        m.push(0, 1);
        m.push(2, 3);
        m.ia5("OAK");
        let inner_bits = m.0.len();

        let mut o = B(Vec::new());
        o.push(0, 1);
        o.push(3, 3);
        o.push(0, 1);
        o.push(0, 1);
        o.push(1, 1);
        if inner_bits < 128 {
            o.push(0, 1);
            o.push(inner_bits as u64, 7);
        } else {
            o.push(0b10, 2);
            o.push(inner_bits as u64, 14);
        }
        o.0.extend(&m.0);
        o.push(0, 1);
        o.push(0, 7);
        let bytes: Vec<u8> =
            o.0.chunks(8)
                .map(|c| {
                    c.iter()
                        .enumerate()
                        .fold(0u8, |v, (i, &b)| v | (b << (7 - i)))
                })
                .collect();

        let v = parse_pdus(&bytes, false).expect("apdu");
        let msg = &v["message"];
        assert_eq!(msg["elements"][0]["element"], "uM80RouteClearance");
        let rc = &msg["route_clearances"][0];
        assert_eq!(rc["destination_airport"], "KSFO");
        assert_eq!(rc["route"][0], "J501");
        assert_eq!(rc["route"][1], "OAK");
    }
}

#[cfg(test)]
mod arg_tests {
    use super::*;

    struct Bb(Vec<u8>);
    impl Bb {
        fn new() -> Self {
            Bb(Vec::new())
        }
        fn push(&mut self, v: u64, n: usize) {
            for k in (0..n).rev() {
                self.0.push(((v >> k) & 1) as u8);
            }
        }
        fn ia5(&mut self, s: &str) {
            for c in s.bytes() {
                self.push(c as u64, 7);
            }
        }
        fn bytes(&self) -> Vec<u8> {
            self.0
                .chunks(8)
                .map(|c| {
                    c.iter()
                        .enumerate()
                        .fold(0u8, |v, (i, &b)| v | (b << (7 - i)))
                })
                .collect()
        }
    }

    fn decode(b: &Bb, ty: &str) -> Vec<String> {
        let bytes = b.bytes();
        let mut store = Vec::new();
        let mut p = Per::new(&bytes, &mut store);
        read_argument(&mut p, ty).expect("argument decodes")
    }

    #[test]
    fn frequency_vhf_decodes() {
        let mut b = Bb::new();
        b.push(1, 2);
        b.push(24300 - 23600, 12);
        assert_eq!(decode(&b, "Frequency"), vec!["121.500 MHz"]);
    }

    #[test]
    fn frequency_hf_and_uhf_and_sat() {
        let mut b = Bb::new();
        b.push(0, 2);
        b.push(8825 - 2850, 15);
        assert_eq!(decode(&b, "Frequency"), vec!["8825 kHz"]);
        let mut b = Bb::new();
        b.push(2, 2);
        b.push(9720 - 9000, 13);
        assert_eq!(decode(&b, "Frequency"), vec!["243.000 MHz"]);
        let mut b = Bb::new();
        b.push(3, 2);
        for c in "123456789012".chars() {
            b.push((c as u8 - b'0') as u64, 4);
        }
        assert_eq!(decode(&b, "Frequency"), vec!["SAT 123456789012"]);
    }

    #[test]
    fn altimeter_english_and_metric() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(2992 - 2200, 10);
        assert_eq!(decode(&b, "Altimeter"), vec!["29.92 inHg"]);
        let mut b = Bb::new();
        b.push(1, 1);
        b.push(10132 - 7500, 13);
        assert_eq!(decode(&b, "Altimeter"), vec!["1013.2 hPa"]);
    }

    #[test]
    fn code_squawk_four_octal_digits() {
        let mut b = Bb::new();
        for d in [7u64, 6, 0, 0] {
            b.push(d, 3);
        }
        assert_eq!(decode(&b, "Code"), vec!["7600"]);
    }

    #[test]
    fn atis_code_single_char() {
        let mut b = Bb::new();
        b.push(b'B' as u64, 7);
        assert_eq!(decode(&b, "ATISCode"), vec!["B"]);
    }

    #[test]
    fn free_text_length_prefixed() {
        let mut b = Bb::new();
        b.push(5 - 1, 8);
        b.ia5("HELLO");
        assert_eq!(decode(&b, "FreeText"), vec!["HELLO"]);
    }

    #[test]
    fn facility_designation_and_facility() {
        let mut b = Bb::new();
        b.push(5 - 4, 3);
        b.ia5("KZAKZ");
        assert_eq!(decode(&b, "FacilityDesignation"), vec!["KZAKZ"]);
        let mut b = Bb::new();
        b.push(0, 1);
        assert_eq!(decode(&b, "Facility"), vec!["(none)"]);
    }

    #[test]
    fn unit_name_frequency_full() {
        let mut b = Bb::new();
        b.push(1, 1);
        b.push(4 - 4, 3);
        b.ia5("KZAK");
        b.push(7 - 3, 4);
        b.ia5("OAKLAND");
        b.push(0, 1);
        b.push(0, 4);
        b.push(1, 2);
        b.push(26830 - 23600, 12);
        assert_eq!(
            decode(&b, "UnitNameFrequency"),
            vec!["KZAK OAKLAND CENTER", "134.150 MHz"]
        );
    }

    #[test]
    fn vertical_rate_english() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(50, 12);
        assert_eq!(decode(&b, "VerticalRate"), vec!["500 FPM"]);
    }

    #[test]
    fn distance_specified_direction_nm() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(20 - 1, 8);
        b.push(0, 4);
        assert_eq!(
            decode(&b, "DistanceSpecifiedDirection"),
            vec!["20 NM", "LEFT"]
        );
    }

    #[test]
    fn traffic_type_enum() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(1, 3);
        assert_eq!(decode(&b, "TrafficType"), vec!["OPPOSITE DIRECTION"]);
    }

    #[test]
    fn clearance_type_enum() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(2, 4);
        assert_eq!(decode(&b, "ClearanceType"), vec!["DEPARTURE"]);
    }

    #[test]
    fn error_information_enum() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(4, 3);
        assert_eq!(
            decode(&b, "ErrorInformation"),
            vec!["INVALID MESSAGE ELEMENT"]
        );
    }

    #[test]
    fn speed_type_triple_and_speed() {
        let mut b = Bb::new();
        for idx in [1u64, 2, 4] {
            b.push(0, 1);
            b.push(idx, 4);
        }
        b.push(6, 3);
        b.push(840 - 500, 12);
        assert_eq!(
            decode(&b, "SpeedTypeSpeedTypeSpeedTypeSpeed"),
            vec!["INDICATED", "TRUE", "MACH", "M0.840"]
        );
    }

    #[test]
    fn position_time_time_two_times() {
        let mut b = Bb::new();
        b.push(2, 3);
        b.ia5("KSFO");
        b.push(10, 5);
        b.push(30, 6);
        b.push(11, 5);
        b.push(45, 6);
        assert_eq!(
            decode(&b, "PositionTimeTime"),
            vec!["KSFO", "10:30", "11:45"]
        );
    }

    #[test]
    fn tofrom_position() {
        let mut b = Bb::new();
        b.push(1, 1);
        b.push(2, 3);
        b.ia5("EGLL");
        assert_eq!(decode(&b, "ToFromPosition"), vec!["FROM", "EGLL"]);
    }

    #[test]
    fn hold_clearance_full() {
        let mut b = Bb::new();
        b.push(1, 1);
        b.push(2, 3);
        b.ia5("KSFO");
        b.push(0, 1);
        b.push(2, 2);
        b.push(250 - 30, 10);
        b.push(0, 1);
        b.push(270 - 1, 9);
        b.push(1, 4);
        b.push(1, 1);
        b.push(5, 4);
        assert_eq!(
            decode(&b, "HoldClearance"),
            vec!["KSFO", "FL250", "270°M", "RIGHT", "5 MIN"]
        );
    }

    #[test]
    fn departure_clearance_head() {
        let mut b = Bb::new();
        b.push(0, 1);
        b.push(0, 1);
        b.push(6 - 2, 3);
        b.ia5("DLH456");
        b.push(2, 3);
        b.ia5("EDDF");
        assert_eq!(
            decode(&b, "DepartureClearance"),
            vec!["DLH456 CLEARED TO EDDF"]
        );
    }

    #[test]
    fn runway_rvr() {
        let mut b = Bb::new();
        b.push(27 - 1, 6);
        b.push(0, 2);
        b.push(0, 1);
        b.push(1200, 13);
        assert_eq!(decode(&b, "RunwayRVR"), vec!["RWY 27L", "1200 FT"]);
    }

    #[test]
    fn position_report_mandatory_only() {
        let mut b = Bb::new();
        b.push(0, 19);
        b.push(2, 3);
        b.ia5("KSFO");
        b.push(12, 5);
        b.push(0, 6);
        b.push(0, 1);
        b.push(2, 2);
        b.push(350 - 30, 10);
        assert_eq!(decode(&b, "PositionReport"), vec!["KSFO 12:00 FL350"]);
    }

    #[test]
    fn remaining_fuel_persons_on_board() {
        let mut b = Bb::new();
        b.push(2, 5);
        b.push(30, 6);
        b.push(150 - 1, 10);
        assert_eq!(
            decode(&b, "RemainingFuelPersonsOnBoard"),
            vec!["02:30", "150"]
        );
    }

    #[test]
    fn version_number() {
        let mut b = Bb::new();
        b.push(5, 4);
        assert_eq!(decode(&b, "VersionNumber"), vec!["v5"]);
    }

    #[test]
    fn uplink_traffic_type_element_walks() {
        let mut m = Bb::new();
        m.push(0, 1);
        m.push(0, 1);
        m.push(7, 6);
        m.push(30, 7);
        m.push(5, 4);
        m.push(10, 5);
        m.push(0, 5);
        m.push(0, 6);
        m.push(0, 6);
        m.push(0, 1);
        m.push(0, 3);
        m.push(166, 8);
        m.push(0, 1);
        m.push(3, 3);
        let inner_bits = m.0.len();
        let mut o = Bb::new();
        o.push(0, 1);
        o.push(3, 3);
        o.push(0, 1);
        o.push(0, 1);
        o.push(1, 1);
        o.push(0, 1);
        o.push(inner_bits as u64, 7);
        o.0.extend(&m.0);
        o.push(0, 1);
        o.push(0, 7);
        let v = parse_pdus(&o.bytes(), false).expect("apdu");
        let el = &v["message"]["elements"][0];
        assert_eq!(el["element"], "uM166TrafficType");
        assert_eq!(el["argument_type"], "TrafficType");
        assert_eq!(el["text"], "DUE TO CONVERGINGTRAFFIC");
    }
}
