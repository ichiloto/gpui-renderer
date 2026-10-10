use crate::app::Output;
use crate::color::{DEFAULT_BACKGROUND, DEFAULT_FOREGROUND};
use crate::display_cache::DisplayRasterCache;
use crate::input;
use crate::protocol::{Anchor, Event, FrameViewport, SourceRect, Sprite};
use crate::retained_paint::{FieldPaint, world_layer_element};
use crate::retained_prepared::PreparedScene;
use crate::retained_protocol::{SpritePivot, Viewport as RetainedViewport};
use crate::state::{PaintItem, RendererState, painted_cells};
use crate::viewport::{PaintRect, ViewportTransform};
use gpui::{
    Context, FocusHandle, IntoElement, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, ObjectFit,
    Render, RenderImage, Window, div, font, img, prelude::*, px, rgb,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub struct Renderer {
    pub state: RendererState,
    pub focus: FocusHandle,
    pub output: Output,
    pub last_viewport: Option<gpui::Size<gpui::Pixels>>,
    pub last_logical_size: Option<(f32, f32)>,
    pub last_device_scale: Option<f32>,
    pub cached_images: Vec<Arc<RenderImage>>,
    pub logical_font_size: Option<f32>,
    /// Measured (advance per em, line height per em), shared by every cell pitch.
    pub font_metrics: Option<(Option<f32>, f32)>,
    pub canvas_fonts: std::collections::HashMap<(u32, u32), f32>,
    pub tile_samples: Rc<RefCell<crate::display_cache::DisplayRasterCache>>,
    pub glyph_fonts: crate::glyph_raster::FontCatalog,
    pub glyph_frame: crate::glyph_cache::GlyphFrame,
    pub glyph_density: Option<f32>,
    pub retained_scene: Option<Arc<PreparedScene>>,
    pub retained_viewport: Option<RetainedViewport>,
    pub retained_observation: Option<crate::diagnostics::FrameTrace>,
    pub retained_needs_reset: bool,
    pub activation: crate::window_activation::ActivationState,
    pub activation_subscription: Option<gpui::Subscription>,
    /// Controls held in a `key_transitions` session.
    pub held: input::HeldControls,
    /// Field sprite and camera slides, presented on the renderer's own clock.
    pub field_motion: crate::field_motion::FieldMotion,
    /// The renderer's monotonic presentation clock for field slides.
    pub field_clock: std::time::Instant,
}

impl Renderer {
    /// Called after ready has been enqueued. Retaining the subscription here
    /// makes observer teardown follow the window entity, not a detached timer.
    pub fn observe_window_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Key transitions need focus loss too: keys released elsewhere never
        // reach this window, so losing focus releases everything held.
        if !self
            .state
            .hello
            .required_capabilities
            .contains(&crate::protocol::Capability::WindowActivation)
            && !self.state.hello.reports_key_transitions()
        {
            return;
        }
        self.report_window_activation(window, cx);
        self.activation_subscription =
            Some(cx.observe_window_activation(window, |view, window, cx| {
                view.report_window_activation(window, cx);
            }));
    }
    fn report_window_activation(&mut self, window: &Window, cx: &mut Context<Self>) {
        let active = window.is_window_active();
        if self
            .output
            .closing
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let Some(event) = self.activation.update(active) else {
            return;
        };
        if !active && self.state.hello.reports_key_transitions() {
            let reset = self.held.reset();
            self.output.emit(reset, cx);
        }
        if self
            .state
            .hello
            .required_capabilities
            .contains(&crate::protocol::Capability::WindowActivation)
        {
            self.output.emit(event, cx);
        }
    }
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

/// Zero-based cell coordinates -> GPUI logical pixels, relative to the origin
/// of whichever cell pitch the sprite belongs to: the field cell for field
/// members, the text grid otherwise.
pub fn sprite_origin(sprite: &Sprite, cell_width: f32, cell_height: f32) -> (f32, f32) {
    match sprite.anchor {
        Anchor::BottomCenter => (
            (sprite.x as f32 + 0.5) * cell_width - sprite.width as f32 / 2.0,
            (sprite.y as f32 + 1.0) * cell_height - sprite.height as f32,
        ),
    }
}

/// Place the image pivot at the cell's ground anchor, then apply its lift.
/// The cell alone still orders and moves the sprite; omission is bottom-center.
pub fn sprite_bounds(
    sprite: &Sprite,
    lift: u32,
    cell: (f32, f32),
    transform: ViewportTransform,
    pivot: Option<SpritePivot>,
) -> PaintRect {
    let (left, top) = sprite_origin(sprite, cell.0, cell.1);
    let pivot = pivot.unwrap_or(SpritePivot { x: 0.5, y: 1.0 });
    transform.surface_rect(
        left + (0.5 - pivot.x as f32) * sprite.width as f32,
        top + (1.0 - pivot.y as f32) * sprite.height as f32 - lift as f32,
        sprite.width as f32,
        sprite.height as f32,
    )
}

impl Render for Renderer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Observes callback entry only: GPUI can coalesce frames or repaint the
        // same snapshot. This is not GPU completion or proof of visible pixels.
        let observation = self.retained_observation.or_else(|| {
            self.state
                .frame
                .as_ref()
                .and_then(|frame| frame.observation)
        });
        self.output
            .diagnostics
            .frame_stage(observation, "render_callback");
        if let Some(frame) = &self.state.frame {
            let retained: std::collections::HashSet<_> = self
                .retained_scene
                .as_ref()
                .map_or(&frame.cached_images, |scene| &scene.images)
                .iter()
                .map(|image| image.id)
                .collect();
            for previous in &self.cached_images {
                if !retained.contains(&previous.id)
                    && let Err(error) = window.drop_image(previous.clone())
                {
                    crate::protocol::diagnostic(format!("cannot retire cached image: {error}"));
                }
            }
            self.cached_images.clone_from(
                self.retained_scene
                    .as_ref()
                    .map_or(&frame.cached_images, |scene| &scene.images),
            );
        }
        let mut active_regions: std::collections::HashSet<_> = self
            .state
            .frame
            .iter()
            .flat_map(|frame| &frame.tile_batches)
            .flat_map(|batch| &batch.regions)
            .map(|region| region.id)
            .collect();
        if let Some(canvas) = self
            .state
            .frame
            .as_ref()
            .and_then(|frame| frame.canvas.as_ref())
        {
            active_regions.extend(
                canvas
                    .images
                    .iter()
                    .chain(&canvas.composites)
                    .map(|image| image.id),
            );
        }
        active_regions.extend(self.glyph_frame.images.values().map(|image| image.id));
        if let Some(scene) = &self.retained_scene {
            active_regions.extend(
                scene
                    .worlds
                    .values()
                    .flat_map(|world| world.get_images())
                    .map(|image| image.id),
            );
        }
        for retired in self
            .tile_samples
            .borrow_mut()
            .begin_frame(&active_regions, &self.glyph_frame.keys)
        {
            if let Err(error) = window.drop_image(retired) {
                crate::protocol::diagnostic(format!("cannot retire tile sample: {error}"));
            }
        }
        let grid = self.state.hello.grid;
        let cw = grid.cell_width as f32;
        let ch = grid.cell_height as f32;
        let (advance, line_em) = *self.font_metrics.get_or_insert_with(|| {
            let text = cx.text_system();
            let font_id = text.resolve_font(&font(FONT_FAMILY));
            let advance = text.ch_advance(font_id, px(1.0)).ok().map(f32::from);
            let line_em = f32::from(text.ascent(font_id, px(1.0)))
                + f32::from(text.descent(font_id, px(1.0)));
            (advance, line_em)
        });
        self.logical_font_size.get_or_insert_with(|| {
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
        let density = transform.scale.max(1.0 / 256.0) * window.scale_factor();
        if self.glyph_density != Some(density) {
            self.glyph_density = Some(density);
            if let Some(frame) = &self.state.frame {
                match crate::glyph_cache::prepare(
                    frame,
                    density,
                    &mut self.glyph_fonts,
                    &mut self.tile_samples.borrow_mut(),
                ) {
                    Ok(glyphs) => self.glyph_frame = glyphs,
                    Err(error) => crate::protocol::diagnostic(format!(
                        "cannot resize glyph rasters; retaining complete generation: {error}"
                    )),
                }
            }
        }
        if self.last_viewport.is_some()
            && (self.last_viewport != Some(viewport)
                || self.last_device_scale != Some(window.scale_factor()))
        {
            self.output.emit(Event::Resized, cx);
        }
        if self.last_viewport != Some(viewport)
            || self.last_logical_size != Some((logical_width, logical_height))
            || self.last_device_scale != Some(window.scale_factor())
        {
            self.last_viewport = Some(viewport);
            self.last_logical_size = Some((logical_width, logical_height));
            self.last_device_scale = Some(window.scale_factor());
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
                let normalized = if this.state.hello.reports_key_transitions() {
                    this.held.press(&event.keystroke)
                } else {
                    input::normalize(&event.keystroke)
                };
                if let Some(trace) = &mut trace {
                    trace.normalized(match &normalized {
                        Some(Event::Key { key, .. }) => Some(key),
                        _ => None,
                    });
                }
                if let Some(event) = normalized {
                    this.output.emit_key(event, trace, cx);
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if this.state.hello.reports_key_transitions()
                    && let Some(event) = this.held.release(&event.keystroke)
                {
                    this.output.emit(event, cx);
                }
            }))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                if this.state.hello.reports_key_transitions()
                    && let Some(event) = this.held.change_modifiers(event.modifiers)
                {
                    this.output.emit(event, cx);
                }
            }))
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(self
                .retained_scene
                .as_ref()
                .and_then(|scene| scene.source.canvas.as_ref())
                .and_then(|canvas| canvas.background.as_ref())
                .map_or(DEFAULT_BACKGROUND, crate::color::ColorSpec::rgb)));
        if transform.scale == 0.0 {
            return crate::render_trace::observe(root, &self.output.diagnostics, observation);
        }
        let now = self.field_clock.elapsed().as_secs_f64();
        let surface = paint_frame(
            FramePaint {
                frame: self.state.frame.as_deref(),
                scene: self.retained_scene.as_ref(),
                viewport: self.retained_viewport.as_ref(),
                grid,
                transform,
                metrics: (advance, line_em),
                motion: &self.field_motion,
                time: now,
                fonts: &mut self.canvas_fonts,
                glyphs: &self.glyph_frame.images,
                samples: &self.tile_samples,
            },
            cx,
        );
        if self.field_motion.is_animating(now) {
            window.request_animation_frame();
        }

        let root = root.child(surface);
        crate::render_trace::observe(root, &self.output.diagnostics, observation)
    }
}

/// Shared complete-scene painter. Runtime and authoring views differ only in clock and input ownership.
pub(crate) struct FramePaint<'a> {
    pub frame: Option<&'a crate::state::PreparedFrame>,
    pub scene: Option<&'a Arc<PreparedScene>>,
    pub viewport: Option<&'a RetainedViewport>,
    pub grid: crate::protocol::Grid,
    pub transform: ViewportTransform,
    pub metrics: (Option<f32>, f32),
    pub motion: &'a crate::field_motion::FieldMotion,
    pub time: f64,
    pub fonts: &'a mut std::collections::HashMap<(u32, u32), f32>,
    pub glyphs: &'a std::collections::HashMap<usize, Arc<RenderImage>>,
    pub samples: &'a Rc<RefCell<DisplayRasterCache>>,
}

pub(crate) fn paint_frame(paint: FramePaint<'_>, cx: &gpui::App) -> gpui::Div {
    paint_frame_with_authoring(paint, cx, None)
}

pub(crate) struct AuthoringPaint<'a> {
    pub layer_opacities: &'a [Option<f32>],
    pub excluded_cells: &'a std::collections::HashSet<(u32, u32)>,
    pub overlay: Option<(i32, gpui::AnyElement)>,
}

impl AuthoringPaint<'_> {
    fn take_overlay_before(&mut self, band: i32) -> Option<gpui::AnyElement> {
        if self
            .overlay
            .as_ref()
            .is_some_and(|(layer, _)| band >= *layer)
        {
            self.overlay.take().map(|(_, element)| element)
        } else {
            None
        }
    }
}

pub(crate) fn paint_frame_with_authoring(
    paint: FramePaint<'_>,
    cx: &gpui::App,
    mut authoring: Option<AuthoringPaint<'_>>,
) -> gpui::Div {
    let grid = paint.grid;
    let (cw, ch) = (grid.cell_width as f32, grid.cell_height as f32);
    let (advance, line_em) = paint.metrics;
    let logical_font_size = fit_cell_font(cw, ch, advance, line_em);
    let transform = paint.transform;
    // The field has its own cell size, carried by the world the retained
    // viewport presents: one terminal cell, in the terminal's shape. UI
    // text keeps the session grid's pitch.
    let field_cell = paint
        .viewport
        .and_then(|view| view.world_id.as_ref())
        .and_then(|id| paint.scene.as_ref()?.get_world(id))
        .map(|world| world.source.cell_size());
    let field_font_size =
        field_cell.map(|(width, height)| fit_cell_font(width, height, advance, line_em));
    let now = paint.time;
    let world_view = paint
        .viewport
        .zip(field_cell)
        .map(|(view, cell)| paint.motion.get_world_viewport(view, cell, now));
    let bounds = transform.surface_bounds();
    let mut surface = create_text_surface(
        bounds,
        logical_font_size * transform.scale,
        ch * transform.scale,
    );
    if let Some(frame) = paint.frame {
        if let Some(canvas) = frame.canvas.as_ref().filter(|_| !frame.canvas_overlay) {
            surface = surface.child(canvas.element(
                transform,
                paint.fonts,
                paint.glyphs,
                paint.samples.clone(),
                cx,
            ));
        } else if frame.canvas.is_none() {
            paint.fonts.clear();
        }
        // World layers interleave with the plan by layer, so a tiles
        // layer above characters paints above their sprites.
        let field_world = world_view
            .as_ref()
            .zip(paint.scene.as_ref())
            .and_then(|(view, scene)| Some((view, scene.get_world(view.world_id.as_ref()?)?)));
        let world_layers: Vec<i32> = field_world.map_or_else(Vec::new, |(_, world)| {
            world.layers.iter().map(|layer| layer.layer).collect()
        });
        let plan_layers: Vec<i32> = frame
            .plan
            .iter()
            .map(|item| frame.get_item_layer(item))
            .collect();
        let projected = field_world.map(|(view, world)| match &authoring {
            Some(options) => crate::retained_paint::project_world_excluding_cells(
                world,
                view,
                options.excluded_cells,
            ),
            None => crate::retained_paint::project_world(world, view),
        });
        for step in crate::retained_paint::merge_paint_order(&world_layers, &plan_layers) {
            let band = match step {
                FieldPaint::World(index) => world_layers[index],
                FieldPaint::Plan(index) => plan_layers[index],
            };
            if let Some(options) = &mut authoring
                && let Some(overlay) = options.take_overlay_before(band)
            {
                surface = surface.child(overlay);
            }
            let item = match step {
                FieldPaint::World(index) => {
                    if let (Some((view, world)), Some(projected)) = (field_world, &projected) {
                        let opacity = match &authoring {
                            Some(options) => options.layer_opacities[index],
                            None => Some(1.0),
                        };
                        let Some(opacity) = opacity else { continue };
                        let layer = world_layer_element(
                            world,
                            index,
                            view,
                            projected,
                            transform,
                            field_font_size.unwrap_or_default(),
                            paint.samples,
                        );
                        surface = if authoring.is_some() {
                            surface
                                .child(div().absolute().size_full().opacity(opacity).child(layer))
                        } else {
                            surface.child(layer)
                        };
                    }
                    continue;
                }
                FieldPaint::Plan(index) => &frame.plan[index],
            };
            let retained_member = paint.viewport.as_ref().filter(|view| match *item {
                PaintItem::Text(index) => {
                    view.text_layer_ids.contains(&frame.text_layers[index].id)
                }
                PaintItem::Sprite(index) => {
                    view.sprite_ids.contains(&frame.sprites[index].sprite.id)
                }
                _ => false,
            });
            let member = frame.viewport.as_ref().filter(|view| match *item {
                PaintItem::Tiles(index) => view.contains_tile(&frame.tile_batches[index].batch.id),
                PaintItem::Text(index) => view.contains_text(&frame.text_layers[index].id),
                PaintItem::Sprite(index) => view.contains_sprite(&frame.sprites[index].sprite.id),
                #[cfg(test)]
                PaintItem::LegacyText => false,
            });
            // Field members share the world's cell: text is one terminal
            // cell per field cell, sprites are placed by whole cells.
            let (pitch_width, pitch_height, item_font) =
                match (retained_member.is_some(), field_cell, field_font_size) {
                    (true, Some((width, height)), Some(font)) => (width, height, font),
                    _ => (cw, ch, logical_font_size),
                };
            let (item_transform, clip) = if let Some(view) = retained_member {
                // Camera-screen members move with the drawn camera; a
                // sprite also moves along its own step.
                let shift = match *item {
                    PaintItem::Text(index) => {
                        paint
                            .motion
                            .get_text_shift(view, &frame.text_layers[index].id, now)
                    }
                    PaintItem::Sprite(index) => paint
                        .motion
                        .get_sprite_shift(&frame.sprites[index].sprite.id, now),
                    _ => (0.0, 0.0),
                };
                let shifted =
                    crate::field_motion::shift_viewport(view, shift, (pitch_width, pitch_height));
                let (content, clip) = if authoring.is_some() {
                    content_geometry_authoring(transform, &shifted, (pitch_width, pitch_height))
                } else {
                    content_geometry_retained(transform, &shifted)
                };
                (content, Some(clip))
            } else {
                member.map_or((transform, None), |view| {
                    let (content, clip) = content_geometry(transform, &view.geometry);
                    (content, Some(clip))
                })
            };
            match *item {
                PaintItem::Tiles(index) => {
                    surface = add_item(
                        surface,
                        crate::tiles::element(
                            frame.tile_batches[index].clone(),
                            grid,
                            item_transform,
                            paint.samples.clone(),
                        ),
                        clip,
                    );
                }
                #[cfg(test)]
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
                    let mut layer = div()
                        .absolute()
                        .size_full()
                        .text_size(px(item_font * item_transform.scale))
                        .line_height(px(pitch_height * item_transform.scale));
                    for text in painted_cells(&frame.text_layers[index]) {
                        let mut painted = cell(
                            text.column,
                            text.row,
                            pitch_width,
                            pitch_height,
                            item_transform,
                        )
                        .bg(rgb(text.background))
                        .text_color(rgb(text.foreground));
                        if let Some(glyph) = text.glyph {
                            painted = painted.child(glyph.to_string());
                        }
                        layer = layer.child(painted);
                    }
                    surface = add_item(surface, layer, clip);
                }

                PaintItem::Sprite(index) => {
                    let item = &frame.sprites[index];
                    // A lift belongs to field sprites, which slide and
                    // follow with the field's own transform.
                    let lift = retained_member
                        .and(paint.scene.as_ref())
                        .map_or(0, |scene| scene.source.get_sprite_lift(&item.sprite.id));
                    let bounds = sprite_bounds(
                        &item.sprite,
                        lift,
                        (pitch_width, pitch_height),
                        item_transform,
                        paint
                            .scene
                            .and_then(|scene| scene.source.get_sprite_pivot(&item.sprite.id)),
                    );
                    if let Some(turned) = paint
                        .scene
                        .and_then(|scene| scene.get_turned_sprite_image(&item.sprite.id))
                    {
                        // The selected crop was turned during retained-scene
                        // preparation. Keep the original destination and
                        // image pivot; sourceRect retains its
                        // established fill behavior, and whole images
                        // retain their established contain behavior.
                        let image = img(turned.clone())
                            .absolute()
                            .left(px(bounds.left))
                            .top(px(bounds.top))
                            .w(px(bounds.width))
                            .h(px(bounds.height));
                        surface = if item.sprite.source_rect.is_some() {
                            add_item(surface, image.object_fit(ObjectFit::Fill), clip)
                        } else {
                            add_item(surface, image, clip)
                        };
                    } else if let Some(rect) = item.sprite.source_rect {
                        let size = item.image.size(0);
                        let sheet = sheet_bounds(
                            rect,
                            size.width.0 as u32,
                            size.height.0 as u32,
                            bounds.width,
                            bounds.height,
                        );
                        surface = add_item(
                            surface,
                            positioned(div(), bounds).overflow_hidden().child(
                                img(item.image.clone())
                                    .object_fit(ObjectFit::Fill)
                                    .absolute()
                                    .left(px(sheet.left))
                                    .top(px(sheet.top))
                                    .w(px(sheet.width))
                                    .h(px(sheet.height)),
                            ),
                            clip,
                        );
                    } else {
                        surface = add_item(
                            surface,
                            img(item.image.clone())
                                .absolute()
                                .left(px(bounds.left))
                                .top(px(bounds.top))
                                .w(px(bounds.width))
                                .h(px(bounds.height)),
                            clip,
                        );
                    }
                }
            }
        }
        if let Some(options) = &mut authoring
            && let Some((_, overlay)) = options.overlay.take()
        {
            surface = surface.child(overlay);
        }
        // Overlay canvas pixels are transparent outside their elements and
        // paint above the complete retained field, sprites, and screen HUD.
        if let Some(canvas) = frame.canvas.as_ref().filter(|_| frame.canvas_overlay) {
            surface = surface.child(canvas.element(
                transform,
                paint.fonts,
                paint.glyphs,
                paint.samples.clone(),
                cx,
            ));
        }
    }

    surface
}

fn add_item(surface: gpui::Div, item: impl IntoElement, clip: Option<PaintRect>) -> gpui::Div {
    if let Some(clip) = clip {
        surface.child(positioned(div(), clip).overflow_hidden().child(item))
    } else {
        surface.child(item)
    }
}

fn content_geometry(
    base: ViewportTransform,
    viewport: &FrameViewport,
) -> (ViewportTransform, PaintRect) {
    let rect = viewport.clip_rect;
    let clip = base.surface_rect(rect.x, rect.y, rect.width, rect.height);
    let content = base.transform_content(
        viewport.scale,
        viewport.origin.x - rect.x,
        viewport.origin.y - rect.y,
    );
    (content, clip)
}

pub(crate) fn content_geometry_retained(
    base: ViewportTransform,
    viewport: &RetainedViewport,
) -> (ViewportTransform, PaintRect) {
    let rect = viewport.clip_rect;
    let clip = base.surface_rect(rect.x, rect.y, rect.width, rect.height);
    let content = base.transform_content(
        viewport.scale,
        viewport.origin.x - rect.x,
        viewport.origin.y - rect.y,
    );
    (content, clip)
}

/// Authoring field members are world-coordinate records, unlike runtime screen-coordinate frames.
pub(crate) fn content_geometry_authoring(
    base: ViewportTransform,
    viewport: &RetainedViewport,
    pitch: (f32, f32),
) -> (ViewportTransform, PaintRect) {
    let mut viewport = viewport.clone();
    viewport.origin.x -= viewport.world_origin.column as f32 * pitch.0 * viewport.scale;
    viewport.origin.y -= viewport.world_origin.row as f32 * pitch.1 * viewport.scale;
    content_geometry_retained(base, &viewport)
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

/// Text grids use their measured font, never the embedding editor's UI font.
pub(crate) fn create_text_surface(
    bounds: PaintRect,
    font_size: f32,
    line_height: f32,
) -> gpui::Div {
    positioned(div(), bounds)
        .overflow_hidden()
        .text_color(rgb(DEFAULT_FOREGROUND))
        .font_family(FONT_FAMILY)
        .text_size(px(font_size))
        .line_height(px(line_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authoring_overlay_interleaves_once_before_above_tiles_and_later_sprites() {
        let excluded = std::collections::HashSet::new();
        let mut options = AuthoringPaint {
            layer_opacities: &[],
            excluded_cells: &excluded,
            overlay: Some((900, div().into_any_element())),
        };
        for band in [-100, 0, 50, 899] {
            assert!(options.take_overlay_before(band).is_none());
        }
        assert!(options.take_overlay_before(900).is_some());
        assert!(options.take_overlay_before(950).is_none());
        assert!(options.overlay.is_none());
    }
    use crate::protocol::Grid;
    use crate::protocol::{ViewportPoint, ViewportRect};
    use crate::retained_paint::should_paint_world_cell;
    use crate::retained_prepared::PreparedWorld;
    use crate::retained_protocol::{WorldCell, WorldRow, WorldTileCell, WorldTileRow};
    use crate::retained_world::ProjectedCell;
    use serde_json::{from_value, json};

    fn create_world_cell(glyph: &str) -> WorldCell {
        WorldCell {
            glyph: glyph.into(),
            foreground: None,
            background: None,
            owner_layer_id: "map:terrain".into(),
        }
    }

    fn prepare_cell_world(
        cells: Vec<WorldCell>,
        covers: Option<&str>,
        available: bool,
    ) -> PreparedWorld {
        let columns = cells.len() as u32;
        let directory = tempfile::tempdir().unwrap();
        if available {
            image::RgbaImage::from_pixel(1, 2, image::Rgba([32, 96, 48, 255]))
                .save(directory.path().join("sheet.png"))
                .unwrap();
        }
        let piece = json!({"sheet":0,"x":0,"y":0,"width":1,"height":2,"left":0,"top":0});
        let mut world = crate::retained_world::World::new(
            from_value(
                json!({"columns":columns,"rows":1,"cellWidth":10,"cellHeight":20,
                "layers":[
                    {"id":"tiles:ground","layer":-100,"kind":"tiles","coversLayerId":covers},
                    {"id":"map:terrain","layer":-99,"kind":"gameplay"},
                    {"id":"map:buildings","layer":-98,"kind":"gameplay"}
                ],
                "tileset":{"tileSize":2,"sheets":["sheet.png"],
                    "tiles":[{"width":1,"frames":[[piece]]}]}}),
            )
            .unwrap(),
        )
        .unwrap();
        world
            .replace_rows(vec![WorldRow { row: 0, cells }])
            .unwrap();
        world
            .replace_tile_rows(
                "tiles:ground",
                vec![WorldTileRow {
                    row: 0,
                    cells: (0..columns)
                        .map(|column| WorldTileCell { column, tile: 0 })
                        .collect(),
                }],
            )
            .unwrap();
        world.validate_complete().unwrap();
        let assets = crate::assets::AssetRoot::new(directory.path()).unwrap();
        PreparedWorld::prepare("synthetic", Arc::new(world), &assets, None)
    }

    fn project_cell_world(world: &PreparedWorld) -> Vec<ProjectedCell> {
        let view = from_value(
            json!({"scale":1,"origin":{"x":0,"y":0},"worldId":"synthetic",
            "clipRect":{"x":0,"y":0,"width":world.source.definition.columns * 10,"height":20}}),
        )
        .unwrap();
        crate::retained_paint::project_world(world, &view).to_vec()
    }

    #[test]
    fn world_owner_cells_leave_unstyled_blanks_transparent_over_other_layer_tiles() {
        for foreground in [None, Some(crate::color::ColorSpec::Ansi16 { index: 2 })] {
            for glyph in [" ", "   ", "\u{2003}"] {
                let mut source = create_world_cell(glyph);
                source.foreground = foreground.clone();
                let world = prepare_cell_world(vec![source], Some("map:buildings"), true);
                let projected = project_cell_world(&world)[0];
                let source = &world.source.get_row(0).unwrap().cells[0];
                assert_eq!(world.source.get_tile("tiles:ground", 0, 0), Some(0));
                assert!(world.get_tile_image(0, 0).is_some());
                assert!(world.has_covering_tile(0, 0, "map:buildings"));
                assert!(!world.has_covering_tile(0, 0, "map:terrain"));
                assert_eq!(
                    world
                        .layers
                        .iter()
                        .map(|layer| layer.layer)
                        .collect::<Vec<_>>(),
                    [-100, -99, -98]
                );
                assert!(
                    !should_paint_world_cell(&world, "map:terrain", projected, source),
                    "unstyled blank owner cell must not cover the tile on another gameplay layer"
                );
                assert_eq!(source.glyph, glyph);
                assert_eq!(source.owner_layer_id, "map:terrain");
                assert!(source.background.is_none());
            }
        }
    }

    #[test]
    fn world_owner_cells_keep_nonblank_glyphs_and_authored_backgrounds() {
        let backgrounds = [
            crate::color::ColorSpec::Ansi16 { index: 0 },
            crate::color::ColorSpec::Rgb {
                r: 17,
                g: 24,
                b: 32,
            },
        ];
        let mut cells: Vec<_> = [".", "\u{754c}", "e\u{301}", "  X  "]
            .into_iter()
            .map(create_world_cell)
            .collect();
        for background in backgrounds.clone() {
            let mut cell = create_world_cell(" ");
            cell.background = Some(background);
            cells.push(cell);
        }
        let world = prepare_cell_world(cells, Some("map:buildings"), true);
        for projected in project_cell_world(&world) {
            let source = &world.source.get_row(0).unwrap().cells[projected.world_column as usize];
            assert!(should_paint_world_cell(
                &world,
                "map:terrain",
                projected,
                source
            ));
        }
        assert_eq!(
            world.source.get_row(0).unwrap().cells[4].background,
            Some(backgrounds[0].clone())
        );
        assert_eq!(
            world.source.get_row(0).unwrap().cells[5].background,
            Some(backgrounds[1].clone())
        );
    }

    #[test]
    fn world_owner_cells_keep_layer_identity_and_available_tile_coverage() {
        for covers in [None, Some("map:terrain"), Some("map:buildings")] {
            let mut cells = vec![create_world_cell("#"), create_world_cell(" ")];
            cells[1].background = Some(crate::color::ColorSpec::Ansi16 { index: 0 });
            let world = prepare_cell_world(cells, covers, true);
            for projected in project_cell_world(&world) {
                let source =
                    &world.source.get_row(0).unwrap().cells[projected.world_column as usize];
                assert_eq!(
                    should_paint_world_cell(&world, "map:terrain", projected, source),
                    covers == Some("map:buildings")
                );
                assert!(!should_paint_world_cell(
                    &world,
                    "map:buildings",
                    projected,
                    source
                ));
            }
        }
    }

    #[test]
    fn world_owner_cells_keep_glyph_fallback_when_tile_art_is_unavailable() {
        let mut cells = vec![
            create_world_cell("#"),
            create_world_cell(" "),
            create_world_cell(" "),
        ];
        cells[2].background = Some(crate::color::ColorSpec::Ansi16 { index: 0 });
        let world = prepare_cell_world(cells, Some("map:terrain"), false);
        assert!(world.get_tile_image(0, 0).is_none());
        let selected: Vec<_> = project_cell_world(&world)
            .into_iter()
            .filter(|projected| {
                let source =
                    &world.source.get_row(0).unwrap().cells[projected.world_column as usize];
                should_paint_world_cell(&world, "map:terrain", *projected, source)
            })
            .map(|projected| projected.world_column)
            .collect();
        assert_eq!(selected, [0, 2]);
    }

    #[test]
    fn selected_text_tile_and_sprite_share_a_clipped_presentation() {
        let grid = Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        };
        let view = FrameViewport {
            scale: 2.0,
            origin: ViewportPoint { x: 120.0, y: 50.0 },
            clip_rect: ViewportRect {
                x: 100.0,
                y: 40.0,
                width: 500.0,
                height: 400.0,
            },
            text_layer_ids: vec![],
            sprite_ids: vec![],
            tile_batch_ids: vec![],
        };
        let base = ViewportTransform::fit(1350.0, 720.0, 675.0, 360.0);
        let (content, clip) = content_geometry(base, &view);
        assert_eq!(
            clip,
            PaintRect {
                left: 50.0,
                top: 20.0,
                width: 250.0,
                height: 200.0
            }
        );
        let cell = cell_bounds(8, 4, 10.0, 20.0, content);
        // Text cells and tile destinations both use this cell geometry.
        assert_eq!(
            cell,
            PaintRect {
                left: 90.0,
                top: 85.0,
                width: 10.0,
                height: 20.0
            }
        );
        let sprite = Sprite {
            id: "player".into(),
            asset: "player.png".into(),
            x: 8,
            y: 4,
            width: 32,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer: 100,
            source_rect: None,
        };
        let (left, top) = sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32);
        let actor = content.surface_rect(left, top, 32.0, 48.0);
        assert_eq!(
            (actor.left + actor.width / 2.0, actor.top + actor.height),
            (cell.left + cell.width / 2.0, cell.top + cell.height)
        );
        assert_eq!(cell_bounds(8, 4, 10.0, 20.0, base).left, 40.0);
    }
    #[test]
    fn text_surfaces_own_the_measured_font_and_scaled_grid_metrics() {
        for scale in [0.5, 1.0, 2.0] {
            let mut surface = create_text_surface(
                PaintRect {
                    left: 0.0,
                    top: 0.0,
                    width: 400.0,
                    height: 240.0,
                },
                15.0 * scale,
                20.0 * scale,
            );
            let text = surface.style().text.as_ref().unwrap();
            assert_eq!(text.font_family, Some(FONT_FAMILY.into()));
            assert_eq!(text.font_size, Some(px(15.0 * scale).into()));
            assert_eq!(text.line_height, Some(px(20.0 * scale).into()));
        }
    }

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
            let (left, top) =
                sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32);
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
    fn field_character_frame_fills_its_square_cell() {
        // A 48-pixel character frame stands on its 48 x 48 field cell and
        // covers it exactly, whatever the text grid's shape.
        let sprite = Sprite {
            id: "player".into(),
            asset: "$Hero.png".into(),
            x: 2,
            y: 1,
            width: 48,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer: 100,
            source_rect: None,
        };
        assert_eq!(sprite_origin(&sprite, 48.0, 48.0), (96.0, 48.0));
    }

    #[test]
    fn a_lift_draws_the_sprite_higher_at_every_scale_without_moving_across() {
        let sprite = Sprite {
            id: "player".into(),
            asset: "$Hero.png".into(),
            x: 2,
            y: 1,
            width: 48,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer: 100,
            source_rect: None,
        };
        for scale in [1.0, 0.75, 0.5] {
            let transform =
                ViewportTransform::fit(480.0, 480.0, 480.0 * scale + 60.0, 480.0 * scale);
            let placed = sprite_bounds(&sprite, 0, (48.0, 48.0), transform, None);
            let lifted = sprite_bounds(&sprite, 6, (48.0, 48.0), transform, None);
            assert_eq!(transform.scale, scale);
            // Unlifted, the frame's bottom edge is the cell's bottom edge.
            assert_eq!(
                placed,
                transform.surface_rect(96.0, 48.0, 48.0, 48.0),
                "{scale}"
            );
            assert_eq!(
                lifted,
                PaintRect {
                    top: placed.top - 6.0 * transform.scale,
                    ..placed
                }
            );
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
        assert_eq!(
            sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32),
            (120.0, 72.0)
        );
        let (left, top) = sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32);
        assert_eq!(
            (left + sprite.width as f32 / 2.0, top + sprite.height as f32),
            (136.0, 120.0)
        );
        sprite.width = 64;
        sprite.height = 96;
        assert_eq!(
            sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32),
            (104.0, 24.0)
        );
        assert_eq!((grid.cell_width, grid.cell_height), (16, 24));
        sprite.x = -1;
        sprite.y = 0;
        assert_eq!(
            sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32),
            (-40.0, -72.0)
        );
    }

    #[test]
    fn sprite_pivot_maps_the_drawn_image_to_ground_after_fit_crop_and_scale() {
        for (width, height, source_width, source_height) in
            [(144, 48, 12, 4), (24, 48, 3, 6), (144, 48, 3, 6)]
        {
            let sprite = Sprite {
                id: "effect".into(),
                asset: "effect.png".into(),
                x: 7,
                y: 9,
                width,
                height,
                anchor: Anchor::BottomCenter,
                layer: 100,
                source_rect: Some(SourceRect {
                    x: source_width,
                    y: 0,
                    width: source_width,
                    height: source_height,
                }),
            };
            for scale in [0.5, 0.75, 1.0] {
                let transform = ViewportTransform::fit(400.0, 400.0, 400.0 * scale, 400.0 * scale);
                let legacy = sprite_bounds(&sprite, 0, (48.0, 48.0), transform, None);
                assert_eq!(
                    legacy,
                    sprite_bounds(
                        &sprite,
                        0,
                        (48.0, 48.0),
                        transform,
                        Some(SpritePivot { x: 0.5, y: 1.0 })
                    )
                );
                for pivot in [
                    SpritePivot { x: 0.0, y: 1.0 },
                    SpritePivot { x: 1.0, y: 0.0 },
                    SpritePivot { x: 0.25, y: 0.625 },
                ] {
                    let drawn = sprite_bounds(&sprite, 6, (48.0, 48.0), transform, Some(pivot));
                    assert!(
                        (drawn.left + drawn.width * pivot.x as f32 - 7.5 * 48.0 * scale).abs()
                            < 1e-4
                    );
                    assert!(
                        (drawn.top + drawn.height * pivot.y as f32 - (10.0 * 48.0 - 6.0) * scale)
                            .abs()
                            < 1e-4
                    );
                    assert_eq!(
                        (drawn.width, drawn.height),
                        (width as f32 * scale, height as f32 * scale)
                    );
                }
            }
        }
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
            let (x, y) = sprite_origin(&sprite, grid.cell_width as f32, grid.cell_height as f32);
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
