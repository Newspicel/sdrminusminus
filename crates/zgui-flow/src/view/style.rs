use zgui::prelude::*;

use super::Background;
use crate::model::Rgba;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlowStyle {
    pub edge: Rgba,
    pub edge_selected: Rgba,
    pub edge_width: f64,
    pub connection: Rgba,
    pub connection_valid: Rgba,
    pub connection_invalid: Rgba,
    pub background: Option<Background>,
    pub minimap_node: Option<Rgba>,
    pub minimap_mask: Option<Rgba>,
}

impl Default for FlowStyle {
    fn default() -> Self {
        Self {
            edge: Rgba::new(0.55, 0.58, 0.65, 0.9),
            edge_selected: Rgba::new(0.55, 0.72, 1.0, 1.0),
            edge_width: 1.5,
            connection: Rgba::new(0.7, 0.72, 0.78, 0.9),
            connection_valid: Rgba::new(0.4, 0.85, 0.6, 1.0),
            connection_invalid: Rgba::new(0.95, 0.45, 0.4, 1.0),
            background: Some(Background::default()),
            minimap_node: None,
            minimap_mask: None,
        }
    }
}

pub const FLOW_SHEET: &str = css!(
    r#"
.flow {
    position: relative;
    overflow: hidden;
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    cursor: grab;
    outline: none;
}

.flow.panning, .flow.panning * { cursor: grabbing; }
.flow.dragging, .flow.dragging * { cursor: grabbing; }
.flow.connecting, .flow.connecting * { cursor: crosshair; }
.flow.over-edge { cursor: pointer; }

.flow__background { color: #4d525e; }
.flow__minimap-canvas { color: #737a8c; }
.flow__minimap-mask { color: rgba(0, 0, 0, 0.45); }

.flow__background, .flow__edges {
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
}

.flow__world {
    position: absolute;
    left: 0;
    top: 0;
    width: 0;
    height: 0;
    transform-origin: 0 0;
}

.flow__node {
    position: absolute;
    cursor: default;
}

.flow__handle {
    position: absolute;
    width: 16px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: crosshair;
    z-index: 2;
}

.flow__handle-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background-color: #8a8f9c;
    border: 1.5px solid #14161a;
    pointer-events: none;
}

.flow__handle:hover .flow__handle-dot,
.flow__handle[data-status="from"] .flow__handle-dot {
    width: 11px;
    height: 11px;
}

.flow__handle[data-status="valid"] .flow__handle-dot { background-color: #5fd490; }
.flow__handle[data-status="invalid"] .flow__handle-dot { background-color: #f07364; }

.flow__handle-label {
    position: absolute;
    top: 1px;
    white-space: nowrap;
    font-size: 9px;
    pointer-events: none;
}

.flow__handle-label[data-side="left"] { left: 14px; }
.flow__handle-label[data-side="right"] { right: 14px; }

.flow__grip {
    position: absolute;
    width: 10px;
    height: 10px;
    z-index: 3;
}

.flow__grip[data-grip="top"] { left: 5px; right: 5px; width: auto; top: -5px; cursor: ns-resize; }
.flow__grip[data-grip="bottom"] { left: 5px; right: 5px; width: auto; bottom: -5px; cursor: ns-resize; }
.flow__grip[data-grip="left"] { top: 5px; bottom: 5px; height: auto; left: -5px; cursor: ew-resize; }
.flow__grip[data-grip="right"] { top: 5px; bottom: 5px; height: auto; right: -5px; cursor: ew-resize; }
.flow__grip[data-grip="top-left"] { left: -5px; top: -5px; cursor: nwse-resize; }
.flow__grip[data-grip="top-right"] { right: -5px; top: -5px; cursor: nesw-resize; }
.flow__grip[data-grip="bottom-right"] { right: -5px; bottom: -5px; cursor: nwse-resize; }
.flow__grip[data-grip="bottom-left"] { left: -5px; bottom: -5px; cursor: nesw-resize; }

.flow__edge-label {
    position: absolute;
    transform: translate(-50%, -50%);
    padding: 1px 5px;
    border-radius: 4px;
    font-size: 10px;
    white-space: nowrap;
    pointer-events: none;
    background-color: #1d2026;
    color: #c9ccd4;
}

.flow__selection {
    position: absolute;
    border: 1px solid rgba(120, 160, 255, 0.8);
    background-color: rgba(120, 160, 255, 0.12);
    pointer-events: none;
}

.flow__minimap {
    position: absolute;
    right: 12px;
    bottom: 12px;
    width: 200px;
    height: 140px;
    border-radius: 8px;
    overflow: hidden;
    background-color: rgba(20, 22, 26, 0.92);
    border: 1px solid #30343c;
    cursor: pointer;
    z-index: 5;
}

.flow__minimap-canvas, .flow__minimap-mask {
    position: absolute;
    left: 0;
    top: 0;
    width: 200px;
    height: 140px;
    pointer-events: none;
}

.flow__icon { width: 14px; height: 14px; pointer-events: none; }

.flow__controls {
    position: absolute;
    left: 12px;
    bottom: 12px;
    display: flex;
    flex-direction: column;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid #30343c;
    background-color: rgba(20, 22, 26, 0.92);
    z-index: 5;
}

.flow__control {
    width: 28px;
    height: 26px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #c9ccd4;
    font-size: 14px;
    cursor: pointer;
}

.flow__control:hover { background-color: #2a2e36; }
"#
);
