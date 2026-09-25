use sdrmm_wire::{
    channel::{ChannelParams, ParamLimit},
    decode::BroadcastStatus,
};
use zgui::prelude::*;

use super::{
    settings::{NumberLimit, limit_of},
    tables::{Options, with_current},
};
use crate::{
    store::Store,
    ui::{
        kit_channel::{NumberSpec, format_hz, number_field, setting_row, toggle_row},
        widgets::{pick, segments},
    },
};

#[derive(Clone, Copy)]
pub struct Ctx {
    pub store: Store,
    pub node: StoredValue<String>,
    pub params: Signal<Option<ChannelParams>>,
    pub limits: StoredValue<Vec<ParamLimit>>,
    pub broadcast: Signal<Option<BroadcastStatus>>,
}

impl Ctx {
    pub fn read<T>(
        self,
        get: impl Fn(&ChannelParams) -> Option<T> + Send + Sync + 'static,
        fallback: T,
    ) -> Signal<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        let params = self.params;
        Signal::derive(move || {
            params
                .get()
                .as_ref()
                .and_then(&get)
                .unwrap_or_else(|| fallback.clone())
        })
    }

    pub fn write(self, set: impl FnOnce(&mut ChannelParams)) {
        let node = self.node.get_value();
        self.store
            .edit_channel(&node, move |settings| set(&mut settings.params));
    }

    #[must_use]
    pub fn limit(self, name: &str) -> NumberLimit {
        self.limits.with_value(|limits| limit_of(limits, name))
    }
}

pub fn select<T>(
    ctx: Ctx,
    label: &'static str,
    title: Option<&'static str>,
    options: Vec<(T, String)>,
    value: Signal<T>,
    set: impl Fn(&mut ChannelParams, T) + Clone + 'static,
) -> AnyView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let chosen = Signal::derive(move || Some(value.get()));
    setting_row(
        label,
        title,
        pick(options, chosen, move |next: T| {
            let set = set.clone();
            ctx.write(move |params| set(params, next));
        }),
    )
}

pub fn listed<T: Copy>(options: Options<T>) -> Vec<(T, String)> {
    options
        .iter()
        .map(|(value, label)| (*value, (*label).to_owned()))
        .collect()
}

pub fn segmented<T>(
    ctx: Ctx,
    label: &'static str,
    title: Option<&'static str>,
    options: Options<T>,
    value: Signal<T>,
    set: impl Fn(&mut ChannelParams, T) + Clone + 'static,
) -> AnyView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    setting_row(
        label,
        title,
        segments(options.to_vec(), value, move |next: T| {
            let set = set.clone();
            ctx.write(move |params| set(params, next));
        }),
    )
}

pub fn toggle(
    ctx: Ctx,
    label: &'static str,
    title: Option<&'static str>,
    value: Signal<bool>,
    set: impl Fn(&mut ChannelParams, bool) + Clone + 'static,
) -> AnyView {
    toggle_row(label, title, value, move |next| {
        let set = set.clone();
        ctx.write(move |params| set(params, next));
    })
}

pub fn number(
    ctx: Ctx,
    label: &'static str,
    title: Option<&'static str>,
    spec: NumberSpec,
    value: Signal<Option<f64>>,
    set: impl Fn(&mut ChannelParams, Option<f64>) + Clone + 'static,
) -> AnyView {
    setting_row(
        label,
        title,
        number_field(value, spec, move |next| {
            let set = set.clone();
            ctx.write(move |params| set(params, next));
        }),
    )
}

pub fn bandwidth(
    ctx: Ctx,
    label: &'static str,
    options_hz: &'static [f64],
    value: Signal<f64>,
    set: impl Fn(&mut ChannelParams, f64) + Clone + 'static,
) -> AnyView {
    let chosen = Signal::derive(move || Some(value.get().round() as u64));
    let body = move || {
        let current = value.get().round() as u64;
        let options = with_current(
            current,
            options_hz
                .iter()
                .map(|hz| (hz.round() as u64, format_hz(*hz)))
                .collect(),
            |hz| format_hz(hz as f64),
        );
        let set = set.clone();
        pick(options, chosen, move |next: u64| {
            let set = set.clone();
            ctx.write(move |params| set(params, next as f64));
        })
    };
    setting_row(label, None, body)
}

pub fn presets(
    ctx: Ctx,
    label: &'static str,
    spec: NumberSpec,
    presets: &'static [(f64, &'static str)],
    value: Signal<f64>,
    set: impl Fn(&mut ChannelParams, f64) + Clone + 'static,
) -> AnyView {
    let keyed = presets
        .iter()
        .map(|(value, label)| (value.to_bits(), *label))
        .collect::<Vec<_>>();
    let chosen = Signal::derive(move || value.get().to_bits());
    let preset_set = set.clone();
    let field_set = set;
    setting_row(
        label,
        None,
        view! {
            {segments(keyed, chosen, move |bits: u64| {
                let set = preset_set.clone();
                ctx.write(move |params| set(params, f64::from_bits(bits)));
            })}
            {number_field(Signal::derive(move || Some(value.get())), spec, move |next| {
                if let Some(next) = next {
                    let set = field_set.clone();
                    ctx.write(move |params| set(params, next));
                }
            })}
        },
    )
}

pub fn service_picker(
    ctx: Ctx,
    max: f64,
    value: Signal<Option<u32>>,
    set: impl Fn(&mut ChannelParams, Option<u32>) + Clone + 'static,
) -> AnyView {
    let services = Signal::derive(move || {
        ctx.broadcast
            .get()
            .map(|status| status.services)
            .unwrap_or_default()
    });
    let body = move || {
        let listed = services.get();
        let set = set.clone();
        if listed.is_empty() {
            let spec = NumberSpec::new("Broadcast service identifier")
                .limit(NumberLimit::new(0.0, max, 1.0))
                .optional("Auto");
            return AnyView::new(number_field(
                Signal::derive(move || value.get().map(f64::from)),
                spec,
                move |next| {
                    let set = set.clone();
                    ctx.write(move |params| set(params, next.map(|id| id as u32)));
                },
            ));
        }
        AnyView::new(pick(
            service_options(&listed, value.get()),
            Signal::derive(move || Some(value.get())),
            move |next: Option<u32>| {
                let set = set.clone();
                ctx.write(move |params| set(params, next));
            },
        ))
    };
    setting_row(
        "Service",
        Some(
            "Select a discovered audio, video or data service; Auto chooses the first playable service",
        ),
        body,
    )
}

#[must_use]
pub fn service_options(
    services: &[sdrmm_wire::decode::BroadcastService],
    value: Option<u32>,
) -> Vec<(Option<u32>, String)> {
    let mut options = vec![(None, String::from("Auto"))];
    options.extend(services.iter().map(|service| {
        let label = if service.label.is_empty() {
            service.id.to_string()
        } else {
            service.label.clone()
        };
        (Some(service.id), label)
    }));
    if let Some(value) = value
        && !services.iter().any(|service| service.id == value)
    {
        options.push((Some(value), value.to_string()));
    }
    options
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::decode::BroadcastService;

    use super::*;

    #[test]
    fn a_broadcast_picker_offers_auto_the_services_heard_and_the_one_held() {
        let services = vec![
            BroadcastService {
                id: 49_569,
                label: String::from("Radio One"),
                ..BroadcastService::default()
            },
            BroadcastService {
                id: 7,
                ..BroadcastService::default()
            },
        ];
        assert_eq!(
            service_options(&services, Some(99)),
            vec![
                (None, String::from("Auto")),
                (Some(49_569), String::from("Radio One")),
                (Some(7), String::from("7")),
                (Some(99), String::from("99")),
            ]
        );
        assert_eq!(service_options(&services, Some(7)).len(), 3);
    }
}
