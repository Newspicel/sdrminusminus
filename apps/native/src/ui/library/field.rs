use sdrmm_wire::about::AboutResponse;
use zgui::{
    canvas::{Brush, ShapeBuilder, zgui_color::Color},
    elements::{DrawCx, kurbo, kurbo::Shape as _},
    prelude::*,
};

use crate::{
    shell::field_link::{handoff_origins, handoff_url},
    store::Store,
    ui::{kit_shell::hint, library::load},
};

const QUIET_ZONE: usize = 2;

const SHEET: &str = css!(
    r#"
.field2 { flex-direction: column; align-items: center; gap: 10px; padding: 14px; }
.field2__qr { width: 208px; height: 208px; border-radius: 4px; }
.field2__url { max-width: 320px; text-align: center; font-family: var(--mono); font-size: 11px; color: var(--ink-dim); }
.field2__note { max-width: 300px; text-align: center; }
"#
);

#[must_use]
pub fn modules(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let dark = code
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    Some((code.width(), dark))
}

fn draw_code(cx: &mut DrawCx<'_>, width: usize, dark: &[bool]) {
    let side = f64::from(cx.size.width.0.min(cx.size.height.0));
    if side <= 0.0 || width == 0 {
        return;
    }
    let cells = (width + QUIET_ZONE * 2) as f64;
    let cell = side / cells;
    let white = Color::srgb(1.0, 1.0, 1.0, 1.0);
    let black = Color::srgb(0.0, 0.0, 0.0, 1.0);
    cx.scene.push(
        ShapeBuilder::new(kurbo::Rect::new(0.0, 0.0, side, side).to_path(0.1))
            .fill(Brush::Solid(white))
            .build(),
    );
    let mut path = kurbo::BezPath::new();
    for (at, on) in dark.iter().enumerate() {
        if !on {
            continue;
        }
        let x = ((at % width) + QUIET_ZONE) as f64 * cell;
        let y = ((at / width) + QUIET_ZONE) as f64 * cell;
        path.extend(kurbo::Rect::new(x, y, x + cell + 0.2, y + cell + 0.2).path_elements(0.1));
    }
    cx.scene
        .push(ShapeBuilder::new(path).fill(Brush::Solid(black)).build());
}

pub fn panel(store: Store) -> impl IntoView {
    install_stylesheet("shell-field", SHEET);
    let about = load::<AboutResponse>(store, String::from("/api/about"));
    let pick = RwSignal::new(0usize);
    let origins = move || {
        let lan = about
            .get()
            .and_then(Result::ok)
            .map(|about| about.lan_addresses.clone())
            .unwrap_or_default();
        handoff_origins(store.api().base(), &lan)
    };
    let url = move || {
        let found = origins();
        let origin = found
            .get(pick.get().min(found.len().saturating_sub(1)))
            .cloned()
            .unwrap_or_else(|| store.api().base().to_owned());
        handoff_url(&origin, store.api().token().get().as_deref())
    };
    let code = Memo::new(move |_| modules(&url()));
    let qr = zgui::elements::canvas()
        .class("field2__qr")
        .draw(move |cx: &mut DrawCx<'_>| {
            if let Some((width, dark)) = code.get() {
                draw_code(cx, width, &dark);
            }
        })
        .into_view();
    let choices = move || {
        let found = origins();
        (found.len() > 1).then(|| {
            let items: Vec<AnyView> = found
                .into_iter()
                .enumerate()
                .map(|(at, origin)| {
                    let host = url::Url::parse(&origin)
                        .ok()
                        .and_then(|parsed| {
                            parsed.host_str().map(|host| match parsed.port() {
                                Some(port) => format!("{host}:{port}"),
                                None => host.to_owned(),
                            })
                        })
                        .unwrap_or(origin);
                    AnyView::new(view! {
                        control(
                            class = "seg__item",
                            class:on = move || pick.get() == at,
                            tabindex = Focus::Sequential,
                            a11y:role = Role::Button,
                            on:click:stop = move |_| pick.set(at)
                        ) {
                            {host}
                        }
                    })
                })
                .collect();
            AnyView::new(view! { row(class = "seg") {{items}} })
        })
    };
    let local_only = move || {
        about
            .get()
            .and_then(Result::ok)
            .is_some_and(|about| about.local_only)
    };
    let no_lan = move || {
        about
            .get()
            .and_then(Result::ok)
            .is_some_and(|about| about.lan_addresses.is_empty())
    };
    view! {
        column(class = "field2") {
            {move || if local_only() {
                AnyView::new(view! {
                    column(class = "field2__note") {
                        {hint("The server only listens on this machine. Start it with --bind 0.0.0.0:8080.")}
                    }
                })
            } else {
                AnyView::new(())
            }}
            box(hidden = local_only) {
                {qr}
            }
            text(class = "field2__url", hidden = local_only) {{url}}
            {move || (!local_only()).then(choices)}
            {move || (!local_only() && no_lan()).then(|| hint("This machine reports no address a phone could reach."))}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_link_encodes_into_a_square_code() {
        let (width, dark) = modules("http://192.168.1.20:8080/field?token=s3cret").expect("a code");
        assert!(width >= 21);
        assert_eq!(dark.len(), width * width);
        assert!(dark.iter().any(|on| *on));
        assert!(dark[0], "a finder pattern starts in the top left corner");
    }
}
