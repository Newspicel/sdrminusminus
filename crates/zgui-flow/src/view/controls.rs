use zgui::prelude::*;

use super::FlowHandle;

const PLUS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" fill="none"/></svg>"#;
const MINUS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" fill="none"/></svg>"#;
const FIT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none"/></svg>"#;

pub fn controls<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
) -> impl IntoView {
    let stop = |ev: &mut EventCx<'_, events::PointerDown>| ev.stop_propagation();
    view! {
        column(class = "flow__controls", on:pointer_down = stop) {
            control(class = "flow__control", a11y:label = "Zoom in", on:click = move |_| flow.zoom_in()) {
                vector(class = "flow__icon", document = PLUS)
            }
            control(class = "flow__control", a11y:label = "Zoom out", on:click = move |_| flow.zoom_out()) {
                vector(class = "flow__icon", document = MINUS)
            }
            control(class = "flow__control", a11y:label = "Fit view", on:click = move |_| flow.fit_view(true)) {
                vector(class = "flow__icon", document = FIT)
            }
        }
    }
}
