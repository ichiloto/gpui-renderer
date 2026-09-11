use crate::app::Output;
use crate::color::{DEFAULT_BACKGROUND, DEFAULT_FOREGROUND};
use crate::input;
use crate::protocol::{Anchor, Grid, Sprite};
use crate::state::{PaintItem, RendererState, painted_cells};
use gpui::{
    Context, FocusHandle, IntoElement, KeyDownEvent, Render, Window, div, img, prelude::*, px, rgb,
};

pub struct Renderer {
    pub state: RendererState,
    pub focus: FocusHandle,
    pub output: Output,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grid = self.state.hello.grid;
        let cw = grid.cell_width as f32;
        let ch = grid.cell_height as f32;
        let mut surface = div()
            .id("presentation-grid")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if let Some(event) = input::normalize(&event.keystroke) {
                    this.output.emit(event, cx);
                }
            }))
            .relative()
            .w(px(grid.columns as f32 * cw))
            .h(px(grid.rows as f32 * ch))
            .overflow_hidden()
            .bg(rgb(DEFAULT_BACKGROUND))
            .text_color(rgb(DEFAULT_FOREGROUND))
            .font_family(if cfg!(target_os = "macos") {
                "Menlo"
            } else {
                "monospace"
            })
            .text_size(px((cw * 0.9).min(ch * 0.75)))
            .line_height(px(ch));
        if let Some(frame) = &self.state.frame {
            for item in &frame.plan {
                match *item {
                    PaintItem::LegacyText => {
                        for (row, text) in frame.text.iter().enumerate() {
                            for (column, glyph) in text.chars().enumerate() {
                                if glyph != ' ' {
                                    surface = surface.child(
                                        cell(column as u32, row as u32, cw, ch)
                                            .child(glyph.to_string()),
                                    );
                                }
                            }
                        }
                    }
                    PaintItem::Text(index) => {
                        for text in painted_cells(&frame.text_layers[index]) {
                            let mut painted = cell(text.column, text.row, cw, ch)
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
                        surface = surface.child(
                            img(item.image.clone())
                                .absolute()
                                .left(px(left))
                                .top(px(top))
                                .w(px(item.sprite.width as f32))
                                .h(px(item.sprite.height as f32)),
                        );
                    }
                }
            }
        }

        surface
    }
}

fn cell(column: u32, row: u32, cw: f32, ch: f32) -> gpui::Div {
    // Cell pitch never depends on glyph advance or font metrics.
    div()
        .absolute()
        .left(px(column as f32 * cw))
        .top(px(row as f32 * ch))
        .w(px(cw))
        .h(px(ch))
        .overflow_hidden()
        .text_center()
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
}
