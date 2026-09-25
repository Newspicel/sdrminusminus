#[allow(unused_imports)]
use super::*;

pub fn face(store: Store, node: String) -> impl IntoView {
    let wired = Signal::derive(move || {
        let graph = store.graph.get();
        binding::sources_of(&graph, &node, "audio")
    });
    let names = move || {
        let sources = wired.get();
        if sources.is_empty() {
            String::from("nothing wired")
        } else {
            sources.join(", ")
        }
    };
    let strength = Signal::derive(move || {
        let graph = store.graph.get();
        let state = store.state.get();
        let devices = binding::device_sets(&graph, &state.device_sets);
        let channels = binding::channels(&graph, &state.device_sets, &devices);
        let levels = store.levels.get();
        wired
            .get()
            .iter()
            .filter_map(|source| {
                let channel = channels.get(source)?;
                let owner = binding::device_node_of(&graph, source)?;
                let set = devices.get(&owner)?;
                levels.get(&(*set, channel.id)).map(|level| level.level_db)
            })
            .fold(-120.0f32, f32::max)
    });

    view! {
        column(class = "face") {
            {row_field("Source", view! { text(class = "mono") {{names}} })}
            {row_field("Level", meter(strength))}
        }
    }
}
