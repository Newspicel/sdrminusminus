use std::sync::OnceLock;

use anyhow::{Context, bail};
use sdrmm_wire::channel::{ChannelParams, ParamLimit};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle,
    Number { integer: bool },
    Choice(Vec<String>),
    Text,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub control: Control,
    pub optional: bool,
}

fn schemas() -> anyhow::Result<&'static Value> {
    static SCHEMAS: OnceLock<Result<Value, String>> = OnceLock::new();
    SCHEMAS
        .get_or_init(|| {
            serde_json::to_value(sdrmm_server::openapi()).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

fn resolve<'a>(root: &'a Value, mut schema: &'a Value) -> anyhow::Result<&'a Value> {
    for _ in 0..16 {
        let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
            return Ok(schema);
        };
        schema = reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
            .with_context(|| format!("unknown settings schema {reference}"))?;
    }
    bail!("settings schema contains a reference cycle")
}

pub fn fields(type_id: &str) -> anyhow::Result<Vec<Field>> {
    let root = schemas()?;
    let variants = root
        .pointer("/components/schemas/ChannelParams/oneOf")
        .and_then(Value::as_array)
        .context("channel settings schema is missing")?;
    let variant = variants
        .iter()
        .find(|variant| {
            variant
                .pointer("/properties/type/enum/0")
                .and_then(Value::as_str)
                == Some(type_id)
        })
        .with_context(|| format!("unknown channel type {type_id}"))?;
    let settings = resolve(root, &variant["properties"]["settings"])?;
    let Some(properties) = settings["properties"].as_object() else {
        if settings["type"] == "object" {
            return Ok(Vec::new());
        }
        bail!("channel settings are not an object");
    };
    properties
        .iter()
        .map(|(name, schema)| field(root, name, schema))
        .collect()
}

fn field(root: &Value, name: &str, schema: &Value) -> anyhow::Result<Field> {
    let mut schema = resolve(root, schema)?;
    let mut optional = false;
    if let Some(choices) = schema["oneOf"].as_array() {
        optional = choices.iter().any(|choice| choice["type"] == "null");
        schema = resolve(
            root,
            choices
                .iter()
                .find(|choice| choice["type"] != "null")
                .context("empty settings choice")?,
        )?;
    }
    let kind = if let Some(types) = schema["type"].as_array() {
        optional |= types.iter().any(|kind| kind == "null");
        types
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null")
            .unwrap_or("")
    } else {
        schema["type"].as_str().unwrap_or("")
    };
    let control = if let Some(choices) = schema["enum"].as_array() {
        Control::Choice(
            choices
                .iter()
                .map(|choice| {
                    choice
                        .as_str()
                        .map(str::to_owned)
                        .context("non-text choice")
                })
                .collect::<anyhow::Result<_>>()?,
        )
    } else {
        match kind {
            "boolean" => Control::Toggle,
            "integer" | "number" => Control::Number {
                integer: kind == "integer",
            },
            "string" => Control::Text,
            _ => bail!("unsupported control for {name}: {kind}"),
        }
    };
    Ok(Field {
        name: name.into(),
        label: label(name),
        control,
        optional,
    })
}

pub fn label(name: &str) -> String {
    let mut words = name
        .split('_')
        .map(|word| match word {
            "hz" => "Hz",
            "us" => "µs",
            "ms" => "ms",
            "db" => "dB",
            "deg" => "°",
            "wpm" => "WPM",
            "crc" => "CRC",
            "ctcss" => "CTCSS",
            "dcs" => "DCS",
            "prn" => "PRN",
            "id" => "ID",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(first) = words.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    words
}

pub fn edited(
    params: &ChannelParams,
    field: &Field,
    value: Value,
    limits: &[ParamLimit],
) -> anyhow::Result<ChannelParams> {
    validate(field, &value, limits)?;
    let mut encoded = serde_json::to_value(params)?;
    encoded["settings"][&field.name] = value;
    let mut edited: ChannelParams =
        serde_json::from_value(encoded).context("invalid decoder setting")?;
    if let ChannelParams::Nfm(params) = &mut edited {
        match field.name.as_str() {
            "tone_mode" => match params.tone_mode {
                sdrmm_wire::channel::NfmToneMode::Ctcss => {
                    params.ctcss_hz.get_or_insert(88.5);
                }
                sdrmm_wire::channel::NfmToneMode::Dcs => {
                    params.dcs_code.get_or_insert(23);
                }
                _ => {}
            },
            "scrambler_mode"
                if params.scrambler_mode == sdrmm_wire::channel::NfmScramblerMode::Inversion =>
            {
                params.inversion_hz.get_or_insert(3300.0);
            }
            _ => {}
        }
    }
    Ok(edited)
}

pub fn visible(params: &ChannelParams, name: &str) -> bool {
    match params {
        ChannelParams::Nfm(params) => match name {
            "ctcss_hz" => params.tone_mode == sdrmm_wire::channel::NfmToneMode::Ctcss,
            "dcs_code" => params.tone_mode == sdrmm_wire::channel::NfmToneMode::Dcs,
            "inversion_hz" => {
                params.scrambler_mode == sdrmm_wire::channel::NfmScramblerMode::Inversion
            }
            _ => true,
        },
        _ => true,
    }
}

pub fn parse(field: &Field, text: &str) -> anyhow::Result<Value> {
    if text.trim().is_empty() && field.optional {
        return Ok(Value::Null);
    }
    match field.control {
        Control::Number { integer: true } => Ok(Value::from(
            text.trim().parse::<i64>().context("enter a whole number")?,
        )),
        Control::Number { integer: false } => {
            let number = text.trim().parse::<f64>().context("enter a number")?;
            Ok(Value::Number(
                serde_json::Number::from_f64(number).context("enter a finite number")?,
            ))
        }
        Control::Text => Ok(Value::String(text.to_owned())),
        _ => bail!("this setting uses a choice control"),
    }
}

fn validate(field: &Field, value: &Value, limits: &[ParamLimit]) -> anyhow::Result<()> {
    if value.is_null() {
        if field.optional {
            return Ok(());
        }
        bail!("{} is required", field.label);
    }
    let valid = match &field.control {
        Control::Toggle => value.is_boolean(),
        Control::Text => value.is_string(),
        Control::Choice(choices) => value
            .as_str()
            .is_some_and(|value| choices.iter().any(|choice| choice == value)),
        Control::Number { integer } => value
            .as_f64()
            .is_some_and(|value| value.is_finite() && (!integer || value.fract() == 0.0)),
    };
    if !valid {
        bail!("invalid {}", field.label);
    }
    if let Some(number) = value.as_f64()
        && let Some(limit) = limits.iter().find(|limit| limit.name == field.name)
        && !(limit.min..=limit.max).contains(&number)
    {
        bail!(
            "{} must be between {} and {}",
            field.label,
            limit.min,
            limit.max
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_decoder_setting_has_a_control_and_roundtrips() {
        let root = schemas().expect("schema");
        for variant in root
            .pointer("/components/schemas/ChannelParams/oneOf")
            .and_then(Value::as_array)
            .expect("variants")
        {
            let kind = variant["properties"]["type"]["enum"][0]
                .as_str()
                .expect("kind");
            let params = ChannelParams::default_for(kind).expect("defaults");
            let encoded = serde_json::to_value(&params).expect("encode");
            let fields = fields(kind).expect(kind);
            for name in encoded["settings"].as_object().expect("settings").keys() {
                assert!(
                    fields.iter().any(|field| field.name == *name),
                    "{kind}.{name}"
                );
            }
            for field in fields {
                let value = encoded["settings"][&field.name].clone();
                assert_eq!(
                    edited(&params, &field, value, &[]).expect(&field.name),
                    params
                );
            }
        }
    }

    #[test]
    fn numbers_reject_nonfinite_fractional_integers_and_out_of_range_values() {
        let params = ChannelParams::default_for("nfm").expect("nfm");
        let fields = fields("nfm").expect("fields");
        let bandwidth = fields
            .iter()
            .find(|field| field.name == "bandwidth_hz")
            .expect("bandwidth");
        assert!(parse(bandwidth, "NaN").is_err());
        assert!(parse(bandwidth, "inf").is_err());
        assert!(parse(bandwidth, "").is_err());
        let limits = vec![ParamLimit {
            name: "bandwidth_hz".into(),
            min: 5000.0,
            max: 25000.0,
            step: None,
        }];
        assert!(edited(&params, bandwidth, Value::from(30000), &limits).is_err());
        let dcs = fields
            .iter()
            .find(|field| field.name == "dcs_code")
            .expect("DCS");
        assert!(parse(dcs, "2.5").is_err());
        assert_eq!(parse(dcs, "").expect("optional"), Value::Null);
    }

    #[test]
    fn changing_one_setting_preserves_the_rest_and_enums_are_checked() {
        let params = ChannelParams::default_for("nfm").expect("nfm");
        let field = fields("nfm")
            .expect("fields")
            .into_iter()
            .find(|field| field.name == "tone_mode")
            .expect("tone mode");
        assert!(edited(&params, &field, Value::from("unknown"), &[]).is_err());
        let changed = edited(&params, &field, Value::from("ctcss"), &[]).expect("change");
        let mut original = serde_json::to_value(params).expect("original");
        original["settings"]["tone_mode"] = Value::from("ctcss");
        original["settings"]["ctcss_hz"] = Value::from(88.5);
        assert_eq!(serde_json::to_value(changed).expect("changed"), original);
    }
}
