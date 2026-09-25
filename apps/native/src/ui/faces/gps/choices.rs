use sdrmm_wire::position::{
    DEFAULT_GPSD_ADDRESS, NmeaDeviceInfo, NmeaDevicesResponse, PositionSource,
};
use zgui::prelude::*;

use super::source::{
    GpsTab, filter_nmea_devices, gps_tabs, nmea_detail, nmea_source, valid_gpsd_address,
};
use crate::{
    store::Store,
    ui::kit_sources::{NumberSpec, Tone, button, draft_field, number_field, segmented},
};

const SEARCH_FROM: usize = 4;

pub type Choose = std::rc::Rc<dyn Fn(PositionSource)>;

pub type Receivers = RwSignal<Option<Result<Vec<NmeaDeviceInfo>, String>>>;

pub fn receivers(store: Store) -> Receivers {
    let found: Receivers = RwSignal::new(None);
    zgui::task::spawn_local(async move {
        let fetched = store
            .api()
            .get::<NmeaDevicesResponse>("/api/position/nmea-devices")
            .await
            .map(|response| response.devices)
            .map_err(|error| error.to_string());
        found.try_set(Some(fetched));
    });
    found
}

pub fn gps_choices(store: Store, choose: Choose) -> impl IntoView {
    let tab = RwSignal::new(GpsTab::Receiver);
    let found = receivers(store);
    let body = move || match tab.get() {
        GpsTab::Receiver => AnyView::new(receiver_choices(found, choose.clone())),
        GpsTab::Network => AnyView::new(gpsd_form(choose.clone())),
        GpsTab::Fixed | GpsTab::Device => AnyView::new(fixed_form(choose.clone())),
    };
    view! {
        column(class = "kit-choices") {
            {segmented(gps_tabs(false), tab.into(), move |next| tab.set(next))}
            {body}
        }
    }
}

fn receiver_choices(found: Receivers, choose: Choose) -> impl IntoView {
    let query = RwSignal::new_local(String::new());
    let listed = Signal::derive(move || found.get().and_then(Result::ok).unwrap_or_default());
    let shown = Signal::derive_local(move || filter_nmea_devices(&listed.get(), &query.get()));
    let pick = choose.clone();
    let list = move || {
        shown
            .get()
            .into_iter()
            .map(|device| {
                let detail = nmea_detail(&device);
                let path = device.path.clone();
                let choose = pick.clone();
                AnyView::new(view! {
                    control(
                        class = "kit-choice",
                        tabindex = Focus::Sequential,
                        a11y:role = Role::Button,
                        on:pointer_down = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation(),
                        on:click:stop = move |_| choose(nmea_source(&path))
                    ) {
                        text {{device.path.clone()}}
                        text(class = "kit-choice__meta", hidden = detail.is_empty()) {{detail}}
                    }
                })
            })
            .collect::<Vec<_>>()
    };
    let said = move || match found.get() {
        None => Some("Looking for receivers…".to_owned()),
        Some(Err(_)) => Some("Serial device discovery failed".to_owned()),
        Some(Ok(all)) if shown.with(Vec::is_empty) => Some(if all.is_empty() {
            "No serial receiver found.".to_owned()
        } else {
            "No receiver matches that.".to_owned()
        }),
        Some(Ok(_)) => None,
    };
    view! {
        column(class = "kit-choices") {
            box(hidden = move || listed.with(Vec::len) < SEARCH_FROM) {
                {draft_field("Search receivers", query, "Search receivers", Signal::derive_local(|| false), || {})}
            }
            column(class = "kit-list") {{list}}
            {move || said().map(|said| AnyView::new(view! { text(class = "kit-note") {{said}} }))}
            {path_form(choose)}
        }
    }
}

fn path_form(choose: Choose) -> impl IntoView {
    let path = RwSignal::new_local(String::new());
    let read = move || {
        let trimmed = path.get_untracked().trim().to_owned();
        if !trimmed.is_empty() {
            choose(nmea_source(&trimmed));
        }
    };
    let empty = Signal::derive(move || path.with(|path| path.trim().is_empty()));
    view! {
        row(class = "kit-pop__row") {
            text(class = "kit-legend") {"Path"}
            {draft_field("Serial device path", path, "/dev/ttyUSB0", Signal::derive_local(|| false), read.clone())}
            {button(|| "Read".to_owned(), Tone::Plain, empty, read)}
        }
    }
}

fn gpsd_form(choose: Choose) -> impl IntoView {
    let address = RwSignal::new_local(DEFAULT_GPSD_ADDRESS.to_owned());
    let invalid = Signal::derive_local(move || !valid_gpsd_address(address.get().trim()));
    let read = move || {
        let typed = address.get_untracked().trim().to_owned();
        if valid_gpsd_address(&typed) {
            choose(PositionSource::Gpsd { address: typed });
        }
    };
    let blocked = Signal::derive(move || !valid_gpsd_address(address.get().trim()));
    view! {
        row(class = "kit-pop__row") {
            text(class = "kit-legend") {"gpsd"}
            {draft_field("GPSD address", address, DEFAULT_GPSD_ADDRESS, invalid, read.clone())}
            {button(|| "Read".to_owned(), Tone::Plain, blocked, read)}
        }
    }
}

fn fixed_form(choose: Choose) -> impl IntoView {
    let lat = RwSignal::new(0.0);
    let lon = RwSignal::new(0.0);
    let latitude = NumberSpec::unit("°").within(-90.0, 90.0).step(0.00001);
    let longitude = NumberSpec::unit("°").within(-180.0, 180.0).step(0.00001);
    view! {
        row(class = "kit-pop__row") {
            text(class = "kit-legend") {"Lat"}
            {number_field("Latitude", lat.into(), latitude, Signal::stored(false), move |value| lat.set(value))}
            text(class = "kit-legend") {"Lon"}
            {number_field("Longitude", lon.into(), longitude, Signal::stored(false), move |value| lon.set(value))}
            {button(|| "Set".to_owned(), Tone::Plain, Signal::stored(false), move || {
                choose(PositionSource::Fixed { lat: lat.get_untracked(), lon: lon.get_untracked(), altitude_m: None });
            })}
        }
    }
}
