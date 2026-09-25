#[allow(unused_imports)]
use super::*;

pub fn face(store: Store, _node: String) -> impl IntoView {
    let rows = move || {
        let decoded = store.decoded.get();
        decoded
            .iter()
            .rev()
            .take(14)
            .map(|record| {
                let when = format::clock(&record.at);
                let what = format!("{} {}", record.event.kind(), record.event.summary());
                view! {
                    row(class = "log__row") {
                        text(class = "log__when") {{when}}
                        text(class = "log__what") {{what}}
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        column(class = "face") {
            column(class = "log") {{rows}}
        }
    }
}
