use sdrmm_wire::{rest::TemplateInfo, state::DeviceSet};

#[must_use]
pub fn supports(template: &TemplateInfo, set: Option<&DeviceSet>) -> bool {
    set.is_some_and(|set| template.supported_devices.contains(&set.device.id()))
}

#[must_use]
pub fn templates_hint(templates: &[TemplateInfo], set: Option<&DeviceSet>) -> Option<String> {
    let Some(set) = set else {
        return Some(String::from("Select a device first."));
    };
    (!templates.is_empty()
        && !templates
            .iter()
            .any(|template| supports(template, Some(set))))
    .then(|| format!("{} cannot run these templates.", set.device.label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(supported: &[&str]) -> TemplateInfo {
        serde_json::from_value(serde_json::json!({
            "id": "airband",
            "name": "Airband",
            "description": "",
            "explainer": "",
            "center_hz": 125e6,
            "sample_rate": 2.4e6,
            "channels": [],
            "min_freq_hz": 118e6,
            "max_freq_hz": 137e6,
            "supported_devices": supported,
        }))
        .expect("a template")
    }

    fn set() -> DeviceSet {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "device": { "driver": "rtlsdr", "key": "0", "label": "RTL-SDR" },
            "capabilities": {
                "freq_ranges": [], "sample_rates": [], "gains": [],
                "antennas": [], "bandwidths": [], "duplex": "rx_only"
            },
            "settings": {},
            "status": "running",
            "channels": [],
        }))
        .expect("a device set")
    }

    #[test]
    fn a_template_runs_on_the_devices_it_names() {
        assert!(supports(&template(&["rtlsdr:0"]), Some(&set())));
        assert!(!supports(&template(&["hackrf:0"]), Some(&set())));
        assert!(!supports(&template(&["rtlsdr:0"]), None));
    }

    #[test]
    fn hints_at_a_missing_or_unfit_device() {
        assert_eq!(
            templates_hint(&[template(&[])], None).as_deref(),
            Some("Select a device first.")
        );
        assert_eq!(
            templates_hint(&[template(&["hackrf:0"])], Some(&set())).as_deref(),
            Some("RTL-SDR cannot run these templates.")
        );
        assert_eq!(
            templates_hint(&[template(&["rtlsdr:0"])], Some(&set())),
            None
        );
        assert_eq!(templates_hint(&[], Some(&set())), None);
    }
}
