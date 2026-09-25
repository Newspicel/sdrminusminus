#[allow(unused_imports)]
use super::*;

pub fn face(store: Store, node: String) -> impl IntoView {
    let set = set_signal(store, node.clone());
    let hz = Signal::derive(move || {
        set.get()
            .and_then(|set| set.settings.center_hz)
            .unwrap_or_default()
    });
    let tune = {
        let node = node.clone();
        move |value: f64| store.tune_device(node.clone(), value)
    };

    let rates = Signal::derive(move || {
        set.get()
            .map(|set| set.capabilities.sample_rates.clone())
            .unwrap_or_default()
    });
    let chosen_rate = Signal::derive(move || set.get().and_then(|set| set.settings.sample_rate));
    let pick_rate = move |value: f64| {
        if let Some(id) = set.get_untracked().map(|set| set.id) {
            store.set_device(
                id,
                DeviceSettings {
                    sample_rate: Some(value),
                    ..DeviceSettings::default()
                },
            );
        }
    };

    let radios = Signal::derive(move || (*store.devices.get()).clone());
    let chosen_radio = {
        let node = node.clone();
        Signal::derive(move || {
            let graph = store.graph.get();
            graph.nodes.iter().find_map(|found| match &found.body {
                NodeBody::Device(device) if found.id == node => {
                    device.device.as_ref().map(|reference| {
                        reference.backend.clone()
                            + ":"
                            + reference.key.as_deref().unwrap_or_default()
                    })
                }
                _ => None,
            })
        })
    };
    let pick_radio = {
        let node = node.clone();
        move |key: String| {
            let Some(info) = store
                .devices
                .get_untracked()
                .iter()
                .find(|info| device_key(info) == key)
                .cloned()
            else {
                return;
            };
            let node = node.clone();
            store.edit_graph(move |graph| {
                if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node)
                    && let NodeBody::Device(device) = &mut found.body
                {
                    device.device = Some(DeviceRef::from_info(&info));
                }
            });
        }
    };

    let dc = Signal::derive(move || {
        set.get()
            .and_then(|set| set.settings.dc_block)
            .unwrap_or(false)
    });
    let toggle_dc = move |on: bool| {
        if let Some(id) = set.get_untracked().map(|set| set.id) {
            store.set_device(
                id,
                DeviceSettings {
                    dc_block: Some(on),
                    ..DeviceSettings::default()
                },
            );
        }
    };

    view! {
        column(class = "face") {
            {dial(hz, tune)}
            {move || {
                let options = radios
                    .get()
                    .iter()
                    .map(|info| (device_key(info), info.label.clone()))
                    .collect::<Vec<_>>();
                row_field("Radio", pick(options, chosen_radio, pick_radio.clone()))
            }}
            {move || {
                let options = rates
                    .get()
                    .iter()
                    .map(|rate| (*rate, format::rate(*rate)))
                    .collect::<Vec<_>>();
                row_field("Rate", pick(options, chosen_rate, pick_rate))
            }}
            {row_field("DC block", check(dc, toggle_dc))}
        }
    }
}

pub(crate) fn device_key(info: &DeviceInfo) -> String {
    format!("{}:{}", info.driver, info.key)
}
