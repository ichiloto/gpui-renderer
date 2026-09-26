//! Visible-only world painting. Camera frames project logical cells; each world
//! cell is one square field cell whatever its text.
use crate::{
    retained_prepared::PreparedWorld,
    retained_protocol::Viewport,
    retained_world::ProjectedCell,
    viewport::{PaintRect, ViewportTransform},
};
use std::sync::Arc;

pub fn project_world(world: &PreparedWorld, viewport: &Viewport) -> Arc<Vec<ProjectedCell>> {
    let mut cells = Vec::new();
    world
        .source
        .project_visible(viewport, |projected, _| cells.push(projected));
    Arc::new(cells)
}

/// A projected world cell's square bounds at the field's own pitch.
pub fn cell_bounds(
    cell: ProjectedCell,
    field_cell: f32,
    base: ViewportTransform,
    viewport: &Viewport,
) -> PaintRect {
    let pitch = field_cell * viewport.scale;
    base.surface_rect(
        viewport.origin.x + cell.screen_column as f32 * pitch,
        viewport.origin.y + cell.screen_row as f32 * pitch,
        pitch,
        pitch,
    )
}
