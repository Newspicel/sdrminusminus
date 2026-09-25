use zgui::prelude::*;
use zgui_flow::{
    interaction::{Effect, Options},
    kurbo::{Point, Size},
    model::{Connection, Edge, Handle, HandleKind, Node, Rgba},
    path::Side,
    view::{FlowHandle, FlowStyle, NodeCx, controls, flow_view, minimap},
};

const SHEET: &str = css!(
    r#"
:root { display: flex; background-color: #14161a; color: #e8eaf0; font-family: system-ui, sans-serif; font-size: 13px; }
.card { width: 100%; height: 100%; border-radius: 8px; background-color: #1d2026; border: 1px solid #30343c; display: flex; flex-direction: column; }
.flow__node.selected .card { border-color: #7aa2ff; }
.card__bar { padding: 6px 10px; border-bottom: 1px solid #30343c; font-size: 11px; letter-spacing: 0.06em; color: #9aa0ad; cursor: grab; }
.card__body { padding: 10px; color: #c9ccd4; }
"#
);

fn card(id: &str, x: f64, y: f64, inputs: &[&str], outputs: &[&str]) -> Node<String> {
    let mut node = Node::new(
        id,
        Point::new(x, y),
        Size::new(180.0, 110.0),
        id.to_uppercase(),
    );
    node.drag_handle = true;
    node.resizable = true;
    for (at, name) in inputs.iter().enumerate() {
        node.handles.push(
            Handle::new(
                *name,
                HandleKind::Target,
                Side::Left,
                44.0 + 18.0 * at as f64,
            )
            .labelled(*name),
        );
    }
    for (at, name) in outputs.iter().enumerate() {
        node.handles.push(
            Handle::new(
                *name,
                HandleKind::Source,
                Side::Right,
                44.0 + 18.0 * at as f64,
            )
            .labelled(*name),
        );
    }
    node
}

fn body(cx: NodeCx<String, ()>) -> impl IntoView {
    let flow = cx.flow;
    let id = cx.id.clone();
    let title = move || {
        flow.nodes
            .with(|nodes| {
                nodes
                    .iter()
                    .find(|node| node.id == id)
                    .map(|node| node.data.clone())
            })
            .unwrap_or_default()
    };
    view! {
        column(class = "card") {
            box(class = "card__bar", {..cx.drag_handle()}) {{title}}
            box(class = "card__body") {"Drag the bar, wire the dots."}
        }
    }
}

fn main() -> Result<(), zgui::Error> {
    app()
        .with_title("zgui-flow")
        .with_size(1200.0, 800.0)
        .with_stylesheet(SHEET)
        .run(|| {
            let flow: FlowHandle<String, ()> = FlowHandle::new(Options::default());
            flow.nodes.set(vec![
                card("source", 40.0, 80.0, &[], &["iq"]),
                card("filter", 320.0, 40.0, &["iq"], &["iq", "audio"]),
                card("speaker", 620.0, 180.0, &["audio"], &[]),
                card("scope", 620.0, -60.0, &["iq"], &[]),
            ]);
            let wire = |from: &str, out: &str, to: &str, input: &str| {
                let mut edge = Edge::new(
                    &Connection {
                        source: from.into(),
                        source_handle: out.into(),
                        target: to.into(),
                        target_handle: input.into(),
                    },
                    (),
                );
                if out == "audio" {
                    edge.color = Some(Rgba::new(0.31, 0.82, 0.63, 0.9));
                    edge.animated = true;
                }
                edge
            };
            flow.edges.set(vec![wire("source", "iq", "filter", "iq"), wire("filter", "audio", "speaker", "audio")]);
            let on_effect = move |effect: &Effect| match effect {
                Effect::Connect(connection) => flow.edges.update(|edges| {
                    if !edges.iter().any(|edge| edge.connection() == *connection) {
                        edges.push(Edge::new(connection, ()));
                    }
                }),
                Effect::Delete { nodes, edges } => {
                    flow.nodes.update(|all| all.retain(|node| !nodes.contains(&node.id)));
                    flow.edges.update(|all| all.retain(|edge| !edges.contains(&edge.id)));
                }
                _ => {}
            };
            let valid = |connection: &Connection| connection.source_handle == connection.target_handle;
            let style = FlowStyle::default();
            view! {
                {flow_view(flow, style, valid, on_effect, body, view! { {controls(flow)} {minimap(flow, style)} })}
            }
        })
}
