use kurbo::{Line, Rect, Shape};
use zgui::{canvas::ShapeBuilder, prelude::*};

use super::{FlowHandle, edges::brush};
use crate::model::Rgba;

const FADE_BELOW: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Dots,
    Lines,
    Cross,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Background {
    pub pattern: Pattern,
    pub gap: f64,
    pub size: f64,
    pub colour: Option<Rgba>,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            pattern: Pattern::Dots,
            gap: 24.0,
            size: 1.2,
            colour: None,
        }
    }
}

fn first(offset: f64, pitch: f64) -> f64 {
    offset.rem_euclid(pitch)
}

pub fn background<T: Send + Sync + 'static, E: Send + Sync + 'static>(
    flow: FlowHandle<T, E>,
    look: Background,
) -> impl IntoView {
    zgui::elements::canvas()
        .class("flow__background")
        .draw(move |cx| {
            let viewport = flow.state.with(|state| state.viewport);
            let width = f64::from(cx.size.width.0);
            let height = f64::from(cx.size.height.0);
            let pitch = look.gap * viewport.zoom;
            if width <= 0.0 || height <= 0.0 || pitch < FADE_BELOW {
                return;
            }
            let brush = brush(look.colour);
            let x0 = first(viewport.x, pitch);
            let y0 = first(viewport.y, pitch);
            match look.pattern {
                Pattern::Lines => {
                    let mut x = x0;
                    while x < width {
                        cx.scene.push(
                            ShapeBuilder::new(Line::new((x, 0.0), (x, height)).to_path(0.1))
                                .stroke(brush.clone(), look.size)
                                .build(),
                        );
                        x += pitch;
                    }
                    let mut y = y0;
                    while y < height {
                        cx.scene.push(
                            ShapeBuilder::new(Line::new((0.0, y), (width, y)).to_path(0.1))
                                .stroke(brush.clone(), look.size)
                                .build(),
                        );
                        y += pitch;
                    }
                }
                Pattern::Dots | Pattern::Cross => {
                    let mut marks = kurbo::BezPath::new();
                    let radius = (look.size * viewport.zoom).max(0.6);
                    let mut y = y0;
                    while y < height {
                        let mut x = x0;
                        while x < width {
                            if look.pattern == Pattern::Dots {
                                marks.extend(
                                    Rect::new(x - radius, y - radius, x + radius, y + radius)
                                        .to_path(0.1),
                                );
                            } else {
                                let arm = radius * 3.0;
                                marks.extend(
                                    Rect::new(x - arm, y - 0.5, x + arm, y + 0.5).to_path(0.1),
                                );
                                marks.extend(
                                    Rect::new(x - 0.5, y - arm, x + 0.5, y + arm).to_path(0.1),
                                );
                            }
                            x += pitch;
                        }
                        y += pitch;
                    }
                    cx.scene.push(ShapeBuilder::new(marks).fill(brush).build());
                }
            }
        })
        .into_view()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_mark_follows_the_pan_and_wraps() {
        assert_eq!(first(10.0, 24.0), 10.0);
        assert_eq!(first(-10.0, 24.0), 14.0);
        assert_eq!(first(50.0, 24.0), 2.0);
    }
}
