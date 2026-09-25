use serde::Serialize;

#[must_use]
pub fn serde_name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

#[must_use]
pub fn signed(value: f64, digits: usize) -> String {
    let sign = if value >= 0.0 { "+" } else { "" };
    format!("{sign}{value:.digits$}")
}

#[must_use]
pub fn flag(value: Option<bool>) -> Option<String> {
    value.map(|on| if on { "yes" } else { "no" }.to_owned())
}

#[must_use]
pub fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if value < 0 { format!("-{out}") } else { out }
}

#[must_use]
pub fn feet(ft: Option<i32>) -> Option<String> {
    ft.map(|ft| format!("{} ft", grouped(i64::from(ft))))
}

#[must_use]
pub fn knots(kt: Option<f64>) -> Option<String> {
    kt.map(|kt| format!("{kt:.1} kt"))
}

#[must_use]
pub fn degrees(deg: Option<f64>) -> Option<String> {
    deg.map(|deg| format!("{}°", deg.round()))
}

#[must_use]
pub fn position(lat: Option<f64>, lon: Option<f64>) -> Option<String> {
    Some(format!("{:.5}, {:.5}", lat?, lon?))
}

#[must_use]
pub fn hex(value: u64, width: usize) -> String {
    format!("0x{value:0width$X}")
}

#[must_use]
pub fn bare_hex(value: u64, width: usize) -> String {
    format!("{value:0width$X}")
}

#[must_use]
pub fn percent(fraction: f64) -> String {
    format!("{}%", (fraction * 100.0).round())
}

#[must_use]
pub fn join(parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_the_way_an_operator_writes_them() {
        assert_eq!(grouped(37_000), "37,000");
        assert_eq!(grouped(-1_088), "-1,088");
        assert_eq!(grouped(999), "999");
        assert_eq!(signed(0.75, 2), "+0.75");
        assert_eq!(signed(-0.2, 1), "-0.2");
        assert_eq!(hex(0x1a, 2), "0x1A");
        assert_eq!(bare_hex(0xa1b2, 5), "0A1B2");
        assert_eq!(
            position(Some(52.52), Some(13.405)).as_deref(),
            Some("52.52000, 13.40500")
        );
        assert_eq!(position(Some(52.52), None), None);
        assert_eq!(
            join(["a".to_owned(), String::new(), "b".to_owned()]),
            "a · b"
        );
    }
}
