use serde::de::DeserializeOwned;

pub(crate) fn from_value<T: DeserializeOwned>(value: &serde_json::Value) -> serde_json::Result<T> {
    serde_json::from_str(&value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_reads_back_as_its_type() {
        let value = serde_json::json!({ "a": 1, "b": [true] });
        let back: std::collections::BTreeMap<String, serde_json::Value> =
            from_value(&value).expect("parse");
        assert_eq!(serde_json::to_value(back).expect("value"), value);
    }

    #[test]
    fn a_mismatched_value_is_an_error() {
        assert!(from_value::<u8>(&serde_json::json!("x")).is_err());
    }
}
