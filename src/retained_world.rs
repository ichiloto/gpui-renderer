//! Retained, source-coordinate map data and the camera's logical-cell projection.
//! Only the visible rows are visited during a scroll. Every world cell is one
//! square field cell, whatever its text.
use crate::retained_protocol::{Viewport, WorldCell, WorldDefinition, WorldRow};
use std::{collections::HashSet, sync::Arc};

/// Most characters a world cell's text may hold.
const MAX_CELL_TEXT_CHARS: usize = 8;

#[derive(Clone, Debug)]
pub struct World {
    pub definition: Arc<WorldDefinition>,
    rows: Vec<Option<Arc<WorldRow>>>,
    complete_rows: usize,
    owner_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedCell {
    pub world_column: u32,
    pub world_row: u32,
    pub screen_column: i32,
    pub screen_row: i32,
}

impl World {
    pub fn new(definition: WorldDefinition) -> Result<Self, String> {
        definition.validate()?;
        let rows = vec![None; definition.rows as usize];
        Ok(Self {
            definition: Arc::new(definition),
            rows,
            complete_rows: 0,
            owner_bytes: 0,
        })
    }

    pub fn replace_rows(&mut self, updates: Vec<WorldRow>) -> Result<(), String> {
        let mut seen = HashSet::new();
        for row in &updates {
            if row.row >= self.definition.rows || !seen.insert(row.row) {
                return Err("world row index is out of bounds or repeated".into());
            }
            if row.cells.len() > self.definition.columns as usize {
                return Err("world row exceeds the logical column extent".into());
            }
            for cell in &row.cells {
                let characters = cell.glyph.chars().count();
                if characters == 0
                    || characters > MAX_CELL_TEXT_CHARS
                    || cell.glyph.chars().any(char::is_control)
                    || !self.definition.layers.iter().any(|layer| {
                        layer.id == cell.owner_layer_id
                            && layer.kind == crate::retained_protocol::WorldLayerKind::Gameplay
                    })
                {
                    return Err("world cell text or owner layer is invalid".into());
                }
            }
        }
        for row in updates {
            let index = row.row as usize;
            if let Some(previous) = &self.rows[index] {
                self.owner_bytes -= row_bytes(previous);
            } else {
                self.complete_rows += 1;
            }
            self.owner_bytes += row_bytes(&row);
            self.rows[index] = Some(Arc::new(row));
        }
        Ok(())
    }

    pub fn get_row(&self, row: u32) -> Option<&WorldRow> {
        self.rows.get(row as usize)?.as_deref()
    }

    pub fn validate_complete(&self) -> Result<(), String> {
        if self.complete_rows != self.rows.len() {
            return Err("world owner rows are incomplete".into());
        }
        Ok(())
    }

    pub fn estimated_bytes(&self) -> usize {
        self.owner_bytes + self.definition.layers.len() * 8192
    }

    /// Side of one square field cell in logical pixels, before viewport scale.
    pub fn cell_size(&self) -> f32 {
        self.definition.cell_size as f32
    }

    /// Terminal text columns in one field cell.
    pub fn cell_columns(&self) -> f32 {
        self.definition.cell_columns as f32
    }

    /// Visits only the cells this camera shows; one world column is one
    /// square screen cell.
    pub fn project_visible(
        &self,
        viewport: &Viewport,
        mut visit: impl FnMut(ProjectedCell, &WorldCell),
    ) {
        let first_x = viewport.world_origin.column.max(0) as u32;
        let first_y = viewport.world_origin.row.max(0) as u32;
        let pad_x = (-viewport.world_origin.column).max(0);
        let pad_y = (-viewport.world_origin.row).max(0);
        let pitch = self.cell_size() * viewport.scale;
        let visible_columns =
            ((viewport.clip_rect.width + (viewport.clip_rect.x - viewport.origin.x).abs()) / pitch)
                .ceil() as u32
                + 2;
        let visible_rows = ((viewport.clip_rect.height
            + (viewport.clip_rect.y - viewport.origin.y).abs())
            / pitch)
            .ceil() as u32
            + 2;
        let end_x = self
            .definition
            .columns
            .min(first_x.saturating_add(visible_columns));
        let end_y = self
            .definition
            .rows
            .min(first_y.saturating_add(visible_rows));
        for y in first_y..end_y {
            let Some(row) = self.get_row(y) else { continue };
            for x in first_x..end_x {
                // Authored rows may be ragged. Their missing trail is the
                // session background, not a new painted owner cell.
                let Some(cell) = row.cells.get(x as usize) else {
                    break;
                };
                visit(
                    ProjectedCell {
                        world_column: x,
                        world_row: y,
                        screen_column: pad_x + (x - first_x) as i32,
                        screen_row: pad_y + (y - first_y) as i32,
                    },
                    cell,
                );
            }
        }
    }
}

fn row_bytes(row: &WorldRow) -> usize {
    row.cells
        .iter()
        .map(|cell| cell.glyph.len() + cell.owner_layer_id.len() + 64)
        .sum()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::from_value;
    use serde_json::json;

    fn cell(glyph: &str) -> serde_json::Value {
        json!({"glyph":glyph,"foreground":null,"background":null,"ownerLayerId":"map:terrain"})
    }

    fn definition(size: u32) -> WorldDefinition {
        from_value(
            json!({"columns":4,"rows":4,"cellSize":size,"cellColumns":2,"layers":[{
                "id":"map:terrain","layer":-100,"kind":"gameplay"
            }]}),
        )
        .unwrap()
    }

    #[test]
    fn row_replacement_updates_cached_bounds() {
        let mut world = World::new(definition(20)).unwrap();
        let row = |index: u32, glyph: &str| {
            from_value(json!({"row":index,"cells":[cell(glyph), cell("..")]})).unwrap()
        };
        world.replace_rows(vec![row(0, "..")]).unwrap();
        assert!(world.validate_complete().is_err());
        world
            .replace_rows((1..4).map(|index| row(index, "..")).collect())
            .unwrap();
        assert!(world.validate_complete().is_ok());
        let original = world.estimated_bytes();
        world.replace_rows(vec![row(0, "界")]).unwrap();
        assert_eq!(world.estimated_bytes(), original + 1);
        for invalid in ["", "\u{7}", "123456789"] {
            assert!(world.replace_rows(vec![row(0, invalid)]).is_err());
        }
    }

    #[test]
    fn field_cells_are_square_at_their_own_pitch() {
        let mut world = World::new(definition(48)).unwrap();
        let row = |index: u32| {
            let cells: Vec<_> = (0..4).map(|_| cell("##")).collect();
            from_value(json!({"row":index,"cells":cells})).unwrap()
        };
        world.replace_rows((0..4).map(row).collect()).unwrap();
        // Two 48-pixel cells fit a 96 x 96 clip on both axes, whatever the
        // session text grid's cell shape is, and each shows its whole text.
        let viewport: Viewport = from_value(json!({"scale":1,
            "origin":{"x":0,"y":0},"worldId":"map",
            "clipRect":{"x":0,"y":0,"width":96,"height":96}
        }))
        .unwrap();
        let mut visible = Vec::new();
        world.project_visible(&viewport, |projected, cell| {
            visible.push((projected, cell.glyph.clone()))
        });
        let columns: HashSet<i32> = visible.iter().map(|(cell, _)| cell.screen_column).collect();
        assert_eq!(columns, HashSet::from([0, 1, 2, 3]));
        assert!(visible.iter().all(|(_, glyph)| glyph == "##"));
        for (size, columns) in [(0, 2), (48, 0), (48, 5)] {
            assert!(
                World::new(
                    from_value(json!({"columns":1,"rows":1,"cellSize":size,
                        "cellColumns":columns,"layers":[{
                        "id":"map:terrain","layer":0,"kind":"gameplay"}]}))
                    .unwrap()
                )
                .is_err()
            );
        }
    }
}
