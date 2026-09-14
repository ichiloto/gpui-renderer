use crate::app::Output;
use crate::color::{DEFAULT_BACKGROUND, DEFAULT_FOREGROUND};
use crate::input;
use crate::protocol::{Anchor, Event, Grid, SourceRect, Sprite};
use crate::state::{PaintItem, RendererState, painted_cells};
use crate::viewport::{PaintRect, ViewportTransform};
use gpui::{
    Context, FocusHandle, IntoElement, KeyDownEvent, ObjectFit, Render, RenderImage, Window, div,
    font, img, prelude::*, px, rgb,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub struct Renderer {
    pub state: RendererState,
    pub focus: FocusHandle,
    pub output: Output,
    pub last_viewport: Option<gpui::Size<gpui::Pixels>>,
    pub last_logical_size: Option<(f32, f32)>,
    pub cached_images: Vec<Arc<RenderImage>>,
    pub logical_font_size: Option<f32>,
    pub canvas_fonts: std::collections::HashMap<(u32, u32), f32>,
    pub tile_samples: Rc<RefCell<crate::tile_sampling::TileSamplingCache>>,
}

pub(crate) const FONT_FAMILY: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "monospace"
};

/// Font em size is derived from measured advance/line metrics; cell pitch is fixed.
pub(crate) fn fit_cell_font(
    cw: f32,
    ch: f32,
    advance_per_em: Option<f32>,
    line_per_em: f32,
) -> f32 {
    if !cw.is_finite() || !ch.is_finite() || cw <= 0.0 || ch <= 0.0 {
        return 0.0;
    }
    let Some(advance) = advance_per_em.filter(|v| v.is_finite() && *v > 0.0) else {
        return (cw * 0.9).min(ch * 0.75);
    };
    if !line_per_em.is_finite() || line_per_em <= 0.0 {
        return (cw * 0.9).min(ch * 0.75);
    }
    // Reserve a small inset for glyph edges; cap the em itself at the cell height.
    (cw * 0.95 / advance).min(ch * 0.95 / line_per_em).min(ch)
}

/// Full-sheet bounds relative to the destination clip. Source coordinates are
/// image pixels; the selected rectangle fills the destination logical size.
pub(crate) fn sheet_bounds(
    rect: SourceRect,
    image_width: u32,
    image_height: u32,
    width: f32,
    height: f32,
) -> PaintRect {
    let sx = width / rect.width as f32;
    let sy = height / rect.height as f32;
    PaintRect {
        left: -(rect.x as f32) * sx,
        top: -(rect.y as f32) * sy,
        width: image_width as f32 * sx,
        height: image_height as f32 * sy,
    }
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
        if let Some(frame) = &self.state.frame {
            let retained: std::collections::HashSet<_> =
                frame.cached_images.iter().map(|image| image.id).collect();
            for previous in &self.cached_images {
                if !retained.contains(&previous.id)
                    && let Err(error) = window.drop_image(previous.clone())
                {
                    crate::protocol::diagnostic(format!("cannot retire cached image: {error}"));
                }
            }
            self.cached_images.clone_from(&frame.cached_images);
        }
        let active_regions = self
            .state
            .frame
            .iter()
            .flat_map(|frame| &frame.tile_batches)
            .flat_map(|batch| &batch.regions)
            .map(|region| region.id)
            .collect();
        for retired in self.tile_samples.borrow_mut().begin_frame(&active_regions) {
            if let Err(error) = window.drop_image(retired) {
                crate::protocol::diagnostic(format!("cannot retire tile sample: {error}"));
            }
        }
        let grid = self.state.hello.grid;
        let cw = grid.cell_width as f32;
        let ch = grid.cell_height as f32;
        let logical_font_size = *self.logical_font_size.get_or_insert_with(|| {
            let text = cx.text_system();
            let font_id = text.resolve_font(&font(FONT_FAMILY));
            let advance = text.ch_advance(font_id, px(1.0)).ok().map(f32::from);
            let line_em = f32::from(text.ascent(font_id, px(1.0)))
                + f32::from(text.descent(font_id, px(1.0)));
            let size = fit_cell_font(cw, ch, advance, line_em);
            self.output.diagnostics.geometry("text_metrics", || {
                vec![
                    ("cell_width", f64::from(cw)),
                    ("cell_height", f64::from(ch)),
                    ("advance_per_em", f64::from(advance.unwrap_or(0.0))),
                    ("line_per_em", f64::from(line_em)),
                    ("logical_font_size", f64::from(size)),
                ]
            });
            size
        });
        let viewport = window.viewport_size();
        let (logical_width, logical_height) = self.state.logical_size();
        let transform = ViewportTransform::fit(
            logical_width,
            logical_height,
            viewport.width.into(),
            viewport.height.into(),
        );
        if self.last_viewport != Some(viewport)
            || self.last_logical_size != Some((logical_width, logical_height))
        {
            self.last_viewport = Some(viewport);
            self.last_logical_size = Some((logical_width, logical_height));
            self.output.diagnostics.geometry("viewport", || {
                let bounds = window.bounds();
                vec![
                    ("outer_left", f32::from(bounds.origin.x).into()),
                    ("outer_top", f32::from(bounds.origin.y).into()),
                    ("outer_width", f32::from(bounds.size.width).into()),
                    ("outer_height", f32::from(bounds.size.height).into()),
                    ("width", f32::from(viewport.width).into()),
                    ("height", f32::from(viewport.height).into()),
                    ("logical_width", logical_width.into()),
                    ("logical_height", logical_height.into()),
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
            .font_family(FONT_FAMILY)
            .text_size(px(logical_font_size * transform.scale))
            .line_height(px(ch * transform.scale));
        if let Some(frame) = &self.state.frame {
            if let Some(canvas) = &frame.canvas {
                surface = surface.child(canvas.element(transform, &mut self.canvas_fonts, cx));
            } else {
                self.canvas_fonts.clear();
            }
            for item in &frame.plan {
                match *item {
                    PaintItem::Tiles(index) => {
                        surface = surface.child(crate::tiles::element(
                            frame.tile_batches[index].clone(),
                            grid,
                            transform,
                            self.tile_samples.clone(),
                        ));
                    }
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
                        if let Some(rect) = item.sprite.source_rect {
                            let size = item.image.size(0);
                            let sheet = sheet_bounds(
                                rect,
                                size.width.0 as u32,
                                size.height.0 as u32,
                                bounds.width,
                                bounds.height,
                            );
                            surface = surface.child(
                                positioned(div(), bounds).overflow_hidden().child(
                                    img(item.image.clone())
                                        .object_fit(ObjectFit::Fill)
                                        .absolute()
                                        .left(px(sheet.left))
                                        .top(px(sheet.top))
                                        .w(px(sheet.width))
                                        .h(px(sheet.height)),
                                ),
                            );
                        } else {
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

pub(crate) fn positioned(element: gpui::Div, bounds: PaintRect) -> gpui::Div {
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
    fn measured_font_fills_cells_without_changing_pitch_or_resize_geometry() {
        // Representative monospace metrics: 0.6em advance, 1.2em ascent+descent.
        let size = fit_cell_font(10.0, 20.0, Some(0.6), 1.2);
        assert!(size > 15.0 && size < 16.0);
        assert!(size > 1.5 * 9.0); // Previous 10x20 cell used a 9px em, not a 9px glyph.
        for (cw, ch, advance, line) in [
            (10.0, 20.0, 0.6, 1.2),
            (6.0, 20.0, 0.6, 1.2),
            (20.0, 10.0, 0.6, 1.2),
            (16.0, 24.0, 0.7, 1.4),
        ] {
            let font = fit_cell_font(cw, ch, Some(advance), line);
            for scale in [1.0, 0.75, 0.5] {
                assert!(font * advance * scale <= cw * scale * 0.95 + 0.00001);
                assert!(font * line * scale <= ch * scale * 0.95 + 0.00001);
                let t = ViewportTransform::fit(
                    135.0 * cw,
                    36.0 * ch,
                    135.0 * cw * scale,
                    36.0 * ch * scale,
                );
                assert_eq!(
                    cell_bounds(8, 4, cw, ch, t),
                    PaintRect {
                        left: 8.0 * cw * scale,
                        top: 4.0 * ch * scale,
                        width: cw * scale,
                        height: ch * scale
                    }
                );
                assert_eq!(
                    cell_bounds(9, 4, cw, ch, t).left - cell_bounds(8, 4, cw, ch, t).left,
                    cw * scale
                );
            }
        }
    }

    #[test]
    fn font_metric_fallback_and_extreme_values_remain_bounded() {
        for advance in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f32::NAN),
            Some(f32::INFINITY),
        ] {
            assert_eq!(fit_cell_font(10.0, 20.0, advance, 1.2), 9.0);
        }
        for line in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(fit_cell_font(10.0, 20.0, Some(0.6), line), 9.0);
        }
        assert_eq!(
            fit_cell_font(10.0, 20.0, Some(f32::MIN_POSITIVE), f32::MIN_POSITIVE),
            20.0
        );
        for (cw, ch) in [
            (0.0, 20.0),
            (10.0, 0.0),
            (f32::NAN, 20.0),
            (10.0, f32::INFINITY),
        ] {
            assert_eq!(fit_cell_font(cw, ch, Some(0.6), 1.2), 0.0);
        }
    }
    #[test]
    fn sheet_crop_and_bottom_center_remain_aligned_across_resize() {
        let grid = Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        };
        let sprite = Sprite {
            id: "player".into(),
            asset: "north.png".into(),
            x: 8,
            y: 4,
            width: 32,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer: 100,
            source_rect: Some(SourceRect {
                x: 512,
                y: 768,
                width: 256,
                height: 256,
            }),
        };
        // North is a non-square 1280x1024 sheet; source pixels must not affect destination feet.
        for scale in [1.0, 0.75, 0.5] {
            let transform =
                ViewportTransform::fit(1350.0, 720.0, 1350.0 * scale + 80.0, 720.0 * scale);
            let (left, top) = sprite_origin(&sprite, grid);
            let destination = transform.surface_rect(left, top, 32.0, 48.0);
            let rect = sprite.source_rect.unwrap();
            let sheet = sheet_bounds(rect, 1280, 1024, destination.width, destination.height);
            assert_eq!(
                sheet,
                PaintRect {
                    left: -64.0 * scale,
                    top: -144.0 * scale,
                    width: 160.0 * scale,
                    height: 192.0 * scale
                }
            );
            assert_eq!(
                (
                    destination.left + destination.width / 2.0 + transform.offset_x,
                    destination.top + destination.height
                ),
                (85.0 * scale + 40.0, 100.0 * scale)
            );
            // Neighboring source cells lie outside the destination clipping rectangle.
            let project = |x: f32, y: f32| {
                (
                    sheet.left + x * sheet.width / 1280.0,
                    sheet.top + y * sheet.height / 1024.0,
                )
            };
            assert_eq!(project(512.0, 768.0), (0.0, 0.0));
            assert_eq!(
                project(768.0, 1024.0),
                (destination.width, destination.height)
            );
            assert!(project(511.0, 768.0).0 < 0.0);
            assert!(project(769.0, 768.0).0 > destination.width);
            let next = sheet_bounds(
                SourceRect { x: 768, ..rect },
                1280,
                1024,
                destination.width,
                destination.height,
            );
            assert_eq!(next.left, sheet.left - destination.width);
            assert_eq!(next.top, sheet.top);
        }
    }
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
            source_rect: None,
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
            source_rect: None,
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
                assert_eq!(cell.foreground, crate::color::ANSI16[14]);
                assert_eq!(cell.background, 0x5a1e64);
            }
        }
    }
}
