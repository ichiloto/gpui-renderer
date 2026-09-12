use crate::app::Output;
use crate::color::{DEFAULT_BACKGROUND, DEFAULT_FOREGROUND};
use crate::input;
use crate::protocol::{Anchor, Event, Grid, Sprite};
use crate::state::{PaintItem, RendererState, painted_cells};
use crate::viewport::{PaintRect, ViewportTransform};
use gpui::{
    Context, FocusHandle, IntoElement, KeyDownEvent, Render, Window, div, img, prelude::*, px, rgb,
};

pub struct Renderer {
    pub state: RendererState,
    pub focus: FocusHandle,
    pub output: Output,
    pub last_viewport: Option<gpui::Size<gpui::Pixels>>,
}

/// Zero-based cell coordinates -> GPUI logical pixels, relative to grid origin.
pub fn sprite_origin(sprite: &Sprite, grid: Grid) -> (f32, f32) {
    match sprite.anchor {
        Anchor::BottomCenter => (
            (sprite.x as f32 + 0.5) * grid.cell_width as f32 - sprite.width as f32 / 2.0,
            (sprite.y as f32 + 1.0) * grid.cell_height as f32 - sprite.height as f32,
        ),
    }
}

impl Render for Renderer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Observes callback entry only: GPUI can coalesce frames or repaint the
        // same snapshot. This is not GPU completion or proof of visible pixels.
        let observation = self
            .state
            .frame
            .as_ref()
            .and_then(|frame| frame.observation);
        self.output
            .diagnostics
            .frame_stage(observation, "render_callback");
        let grid = self.state.hello.grid;
        let cw = grid.cell_width as f32;
        let ch = grid.cell_height as f32;
        let viewport = window.viewport_size();
        let transform = ViewportTransform::fit(
            grid.columns as f32 * cw,
            grid.rows as f32 * ch,
            viewport.width.into(),
            viewport.height.into(),
        );
        if self.last_viewport != Some(viewport) {
            self.last_viewport = Some(viewport);
            self.output.diagnostics.geometry("viewport", || {
                let bounds = window.bounds();
                vec![
                    ("outer_left", f32::from(bounds.origin.x).into()),
                    ("outer_top", f32::from(bounds.origin.y).into()),
                    ("outer_width", f32::from(bounds.size.width).into()),
                    ("outer_height", f32::from(bounds.size.height).into()),
                    ("width", f32::from(viewport.width).into()),
                    ("height", f32::from(viewport.height).into()),
                    ("scale", transform.scale.into()),
                    ("offset_x", transform.offset_x.into()),
                    ("offset_y", transform.offset_y.into()),
                    ("presented_width", transform.presented_width.into()),
                    ("presented_height", transform.presented_height.into()),
                ]
            });
        }
        let root = div()
            .id("presentation-viewport")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let mut trace = this
                    .output
                    .diagnostics
                    .native_key(&event.keystroke.key, event.is_held);
                let normalized = input::normalize(&event.keystroke);
                if let Some(trace) = &mut trace {
                    trace.normalized(match &normalized {
                        Some(Event::Key { key }) => Some(key),
                        _ => None,
                    });
                }
                if let Some(event) = normalized {
                    this.output.emit_key(event, trace, cx);
                }
            }))
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(DEFAULT_BACKGROUND));
        if transform.scale == 0.0 {
            return crate::render_trace::observe(root, &self.output.diagnostics, observation);
        }
        let bounds = transform.surface_bounds();
        let mut surface = positioned(div(), bounds)
            .overflow_hidden()
            .text_color(rgb(DEFAULT_FOREGROUND))
            .font_family(if cfg!(target_os = "macos") {
                "Menlo"
            } else {
                "monospace"
            })
            .text_size(px((cw * 0.9).min(ch * 0.75) * transform.scale))
            .line_height(px(ch * transform.scale));
        if let Some(frame) = &self.state.frame {
            for item in &frame.plan {
                match *item {
                    PaintItem::LegacyText => {
                        for (row, text) in frame.text.iter().enumerate() {
                            for (column, glyph) in text.chars().enumerate() {
                                if glyph != ' ' {
                                    surface = surface.child(
                                        cell(column as u32, row as u32, cw, ch, transform)
                                            .child(glyph.to_string()),
                                    );
                                }
                            }
                        }
                    }
                    PaintItem::Text(index) => {
                        for text in painted_cells(&frame.text_layers[index]) {
                            let mut painted = cell(text.column, text.row, cw, ch, transform)
                                .bg(rgb(text.background))
                                .text_color(rgb(text.foreground));
                            if let Some(glyph) = text.glyph {
                                painted = painted.child(glyph.to_string());
                            }
                            surface = surface.child(painted);
                        }
                    }

                    PaintItem::Sprite(index) => {
                        let item = &frame.sprites[index];
                        let (left, top) = sprite_origin(&item.sprite, grid);
                        let bounds = transform.surface_rect(
                            left,
                            top,
                            item.sprite.width as f32,
                            item.sprite.height as f32,
                        );
                        surface = surface.child(
                            img(item.image.clone())
                                .absolute()
                                .left(px(bounds.left))
                                .top(px(bounds.top))
                                .w(px(bounds.width))
                                .h(px(bounds.height)),
                        );
                    }
                }
            }
        }

        let root = root.child(surface);
        crate::render_trace::observe(root, &self.output.diagnostics, observation)
    }
}

fn cell(column: u32, row: u32, cw: f32, ch: f32, transform: ViewportTransform) -> gpui::Div {
    // Cell pitch never depends on glyph advance or font metrics.
    positioned(div(), cell_bounds(column, row, cw, ch, transform))
        .overflow_hidden()
        .text_center()
}

fn cell_bounds(column: u32, row: u32, cw: f32, ch: f32, transform: ViewportTransform) -> PaintRect {
    transform.surface_rect(column as f32 * cw, row as f32 * ch, cw, ch)
}

fn positioned(element: gpui::Div, bounds: PaintRect) -> gpui::Div {
    element
        .absolute()
        .left(px(bounds.left))
        .top(px(bounds.top))
        .w(px(bounds.width))
        .h(px(bounds.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bottom_center_places_feet_on_target_cell() {
        let grid = Grid {
            columns: 80,
            rows: 24,
            cell_width: 16,
            cell_height: 24,
        };
        let mut sprite = Sprite {
            id: "test".into(),
            asset: "test.png".into(),
            x: 8,
            y: 4,
            width: 32,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer: 100,
        };
        assert_eq!(sprite_origin(&sprite, grid), (120.0, 72.0));
        let (left, top) = sprite_origin(&sprite, grid);
        assert_eq!(
            (left + sprite.width as f32 / 2.0, top + sprite.height as f32),
            (136.0, 120.0)
        );
        sprite.width = 64;
        sprite.height = 96;
        assert_eq!(sprite_origin(&sprite, grid), (104.0, 24.0));
        assert_eq!((grid.cell_width, grid.cell_height), (16, 24));
        sprite.x = -1;
        sprite.y = 0;
        assert_eq!(sprite_origin(&sprite, grid), (-40.0, -72.0));
    }

    #[test]
    fn bottom_center_tracks_the_same_cell_at_every_scale_and_offset() {
        let grid = Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        };
        let sprite = Sprite {
            id: "test".into(),
            asset: "test.png".into(),
            x: 8,
            y: 4,
            width: 32,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer: 100,
        };
        for scale in [1.0, 0.75, 0.5] {
            let t = ViewportTransform::fit(1350.0, 720.0, 1350.0 * scale + 80.0, 720.0 * scale);
            let (x, y) = sprite_origin(&sprite, grid);
            let rect = t.viewport_rect(x, y, 32.0, 48.0);
            assert_eq!(t.scale, scale);
            assert_eq!(t.offset_x, 40.0);
            assert_eq!(
                (rect.left + rect.width / 2.0, rect.top + rect.height),
                (40.0 + 85.0 * scale, 100.0 * scale)
            );
            assert_eq!((rect.width, rect.height), (32.0 * scale, 48.0 * scale));
        }
        assert_eq!(
            (sprite.x, sprite.y, sprite.width, sprite.height),
            (8, 4, 32, 48)
        );
    }

    #[test]
    fn styled_glyph_and_explicit_space_keep_scaled_cell_pitch_and_opaque_colours() {
        let layer: crate::protocol::TextLayer = serde_json::from_value(serde_json::json!({
            "id":"ui", "layer":1000, "runs":[{"row":4,"column":8,"text":"X ",
                "foreground":{"kind":"ansi16","index":14},
                "background":{"kind":"rgb","r":90,"g":30,"b":100}}]
        }))
        .unwrap();
        for scale in [1.0, 0.5] {
            let t = ViewportTransform::fit(1350.0, 720.0, 1350.0 * scale, 720.0 * scale + 60.0);
            let cells: Vec<_> = painted_cells(&layer).collect();
            assert_eq!(cells[0].glyph, Some('X'));
            assert_eq!(cells[1].glyph, None);
            for (index, cell) in cells.iter().enumerate() {
                let bounds = cell_bounds(cell.column, cell.row, 10.0, 20.0, t);
                assert_eq!(
                    bounds,
                    PaintRect {
                        left: (80.0 + index as f32 * 10.0) * scale,
                        top: 80.0 * scale,
                        width: 10.0 * scale,
                        height: 20.0 * scale
                    }
                );
                assert_eq!(bounds.top + t.offset_y, 80.0 * scale + 30.0);
                assert_eq!(cell.foreground, 0x00ffff);
                assert_eq!(cell.background, 0x5a1e64);
            }
        }
    }
}
