//! Retained, source-coordinate map data and the camera's logical-cell projection.
//! Only the visible rows are visited during a scroll. Styled owner glyphs remain
//! available when a tile crop cannot be painted at the current camera position.
use crate::retained_protocol::{
    MAX_WORLD_TILE_CELLS, Viewport, WorldCell, WorldDefinition, WorldRow, WorldTileRow,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct World {
    pub definition: Arc<WorldDefinition>,
    rows: Vec<Option<Arc<WorldRow>>>,
    tiles: HashMap<String, Vec<Option<Arc<WorldTileRow>>>>,
    complete_rows: usize,
    owner_bytes: usize,
    tile_cells: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedCell {
    pub world_column: u32,
    pub world_row: u32,
    pub screen_column: i32,
    pub screen_row: i32,
    pub tile_eligible: bool,
}

impl World {
    pub fn new(definition: WorldDefinition) -> Result<Self, String> {
        definition.validate()?;
        let rows = vec![None; definition.rows as usize];
        let tiles = definition
            .layers
            .iter()
            .map(|layer| (layer.id.clone(), vec![None; definition.rows as usize]))
            .collect();
        Ok(Self {
            definition: Arc::new(definition),
            rows,
            tiles,
            complete_rows: 0,
            owner_bytes: 0,
            tile_cells: 0,
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
                if cell.glyph.chars().count() != 1
                    || cell.glyph.chars().any(char::is_control)
                    || !(1..=16).contains(&cell.display_width)
                    || !self.definition.layers.iter().any(|layer| {
                        layer.id == cell.owner_layer_id
                            && layer.kind == crate::retained_protocol::WorldLayerKind::Gameplay
                    })
                {
                    return Err("world owner glyph, width or layer is invalid".into());
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

    pub fn replace_tile_rows(
        &mut self,
        layer_id: &str,
        mut updates: Vec<WorldTileRow>,
    ) -> Result<(), String> {
        let layer = self
            .definition
            .layers
            .iter()
            .find(|layer| layer.id == layer_id)
            .ok_or("world tile row references an unknown layer")?;
        if layer.sources.is_empty() {
            return Err("world tile row references a layer without a crop catalog".into());
        }
        let mut seen = HashSet::new();
        for row in &updates {
            if row.row >= self.definition.rows || !seen.insert(row.row) {
                return Err("world tile row index is out of bounds or repeated".into());
            }
            let mut columns = HashSet::new();
            for cell in &row.cells {
                if cell.column >= self.definition.columns
                    || cell.source as usize >= layer.sources.len()
                    || !columns.insert(cell.column)
                {
                    return Err("world tile cell is out of bounds or repeated".into());
                }
            }
        }
        let rows = self
            .tiles
            .get_mut(layer_id)
            .expect("validated layer exists");
        for row in &mut updates {
            row.cells.sort_unstable_by_key(|cell| cell.column);
        }
        let next_count = updates.iter().fold(self.tile_cells, |count, row| {
            count
                - rows[row.row as usize]
                    .as_ref()
                    .map_or(0, |previous| previous.cells.len())
                + row.cells.len()
        });
        if next_count > MAX_WORLD_TILE_CELLS {
            return Err("world exceeds 1048576 tile candidates".into());
        }
        for row in updates {
            let index = row.row as usize;
            rows[index] = Some(Arc::new(row));
        }
        self.tile_cells = next_count;
        Ok(())
    }

    pub fn get_row(&self, row: u32) -> Option<&WorldRow> {
        self.rows.get(row as usize)?.as_deref()
    }

    pub fn get_tile_row(&self, layer_id: &str, row: u32) -> Option<&WorldTileRow> {
        self.tiles.get(layer_id)?.get(row as usize)?.as_deref()
    }

    pub fn validate_complete(&self) -> Result<(), String> {
        if self.complete_rows != self.rows.len() {
            return Err("world owner rows are incomplete".into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn tile_count(&self) -> usize {
        self.tile_cells
    }

    pub fn estimated_bytes(&self) -> usize {
        self.tile_cells * 16 + self.owner_bytes + self.definition.layers.len() * 8192
    }

    /// Side of one square field cell in logical pixels, before viewport scale.
    pub fn cell_size(&self) -> f32 {
        self.definition.cell_size as f32
    }

    /// Visits only logical columns selected by this camera. A wide glyph
    /// advances the display column, but not the world-column selection index.
    pub fn project_visible(
        &self,
        viewport: &Viewport,
        mut visit: impl FnMut(ProjectedCell, &WorldCell),
    ) {
        let first_x = viewport.world_origin.column.max(0) as u32;
        let first_y = viewport.world_origin.row.max(0) as u32;
        let pad_x = (-viewport.world_origin.column).max(0);
        let pad_y = (-viewport.world_origin.row).max(0);
        let cell_width = self.cell_size() * viewport.scale;
        let cell_height = cell_width;
        let visible_columns = ((viewport.clip_rect.width
            + (viewport.clip_rect.x - viewport.origin.x).abs())
            / cell_width)
            .ceil() as u32
            + 2;
        let visible_rows = ((viewport.clip_rect.height
            + (viewport.clip_rect.y - viewport.origin.y).abs())
            / cell_height)
            .ceil() as u32
            + 2;
        let display_limit = ((viewport.clip_rect.x + viewport.clip_rect.width - viewport.origin.x)
            / cell_width)
            .floor()
            .max(0.0) as i32;
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
            let mut display_column = pad_x;
            for x in first_x..end_x {
                // Authored rows may be ragged. Their missing trail is the
                // session background, not a new painted owner cell.
                let Some(cell) = row.cells.get(x as usize) else {
                    break;
                };
                if display_column + i32::from(cell.display_width) > display_limit {
                    break;
                }
                let logical_column = pad_x + (x - first_x) as i32;
                visit(
                    ProjectedCell {
                        world_column: x,
                        world_row: y,
                        screen_column: display_column,
                        screen_row: pad_y + (y - first_y) as i32,
                        tile_eligible: display_column == logical_column && cell.display_width == 1,
                    },
                    cell,
                );
                display_column += i32::from(cell.display_width);
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

    #[test]
    fn row_replacement_updates_cached_bounds_and_sorts_tile_crops() {
        let definition = from_value(json!({"columns":2,"rows":2,"cellSize":20,"layers":[{
            "id":"map:terrain","layer":-100,"kind":"gameplay",
            "asset":"tiles.png","sources":[{"x":0,"y":0,"width":1,"height":1}]
        }]}))
        .unwrap();
        let mut world = World::new(definition).unwrap();
        let make_row = |row, glyph| {
            from_value(json!({"row":row,"cells":[
                {"glyph":glyph,"foreground":null,"background":null,
                    "displayWidth":1,"ownerLayerId":"map:terrain"},
                {"glyph":".","foreground":null,"background":null,
                    "displayWidth":1,"ownerLayerId":"map:terrain"}
            ]}))
            .unwrap()
        };
        world.replace_rows(vec![make_row(0, ".")]).unwrap();
        assert!(world.validate_complete().is_err());
        world.replace_rows(vec![make_row(1, ".")]).unwrap();
        assert!(world.validate_complete().is_ok());
        world
            .replace_rows(vec![
                from_value(json!({"row":1,"cells":[
                    {"glyph":".","foreground":null,"background":null,
                        "displayWidth":1,"ownerLayerId":"map:terrain"}
                ]}))
                .unwrap(),
            ])
            .unwrap();
        let viewport: Viewport = from_value(json!({"scale":1,
            "origin":{"x":0,"y":0},"worldId":"map",
            "worldOrigin":{"column":0,"row":0},
            "clipRect":{"x":0,"y":0,"width":20,"height":40}
        }))
        .unwrap();
        let mut visible = Vec::new();
        world.project_visible(&viewport, |cell, _| visible.push(cell));
        assert_eq!(visible.iter().filter(|cell| cell.world_row == 1).count(), 1);
        let original = world.estimated_bytes();
        world.replace_rows(vec![make_row(0, "界")]).unwrap();
        assert_eq!(world.estimated_bytes(), original + 2);

        world
            .replace_tile_rows(
                "map:terrain",
                vec![
                    from_value(json!({
                        "row":0,"cells":[{"column":1,"source":0},{"column":0,"source":0}]
                    }))
                    .unwrap(),
                ],
            )
            .unwrap();
        assert_eq!(world.tile_count(), 2);
        assert_eq!(
            world.get_tile_row("map:terrain", 0).unwrap().cells[0].column,
            0
        );
        world
            .replace_tile_rows(
                "map:terrain",
                vec![
                    from_value(json!({
                        "row":0,"cells":[{"column":1,"source":0}]
                    }))
                    .unwrap(),
                ],
            )
            .unwrap();
        assert_eq!(world.tile_count(), 1);
        assert_eq!(world.estimated_bytes(), original + 2 + 16);
    }

    #[test]
    fn field_cells_are_square_at_their_own_pitch() {
        let definition = from_value(json!({"columns":4,"rows":4,"cellSize":48,"layers":[{
            "id":"map:terrain","layer":-100,"kind":"gameplay"
        }]}))
        .unwrap();
        let mut world = World::new(definition).unwrap();
        let row = |row: u32| {
            let cells: Vec<_> = (0..4)
                .map(|_| {
                    json!({"glyph":".","foreground":null,"background":null,
                        "displayWidth":1,"ownerLayerId":"map:terrain"})
                })
                .collect();
            from_value(json!({"row":row,"cells":cells})).unwrap()
        };
        world.replace_rows((0..4).map(row).collect()).unwrap();
        // Two 48-pixel cells fit a 96 x 96 clip on both axes, whatever the
        // session text grid's cell shape is.
        let viewport: Viewport = from_value(json!({"scale":1,
            "origin":{"x":0,"y":0},"worldId":"map",
            "clipRect":{"x":0,"y":0,"width":96,"height":96}
        }))
        .unwrap();
        let mut visible = Vec::new();
        world.project_visible(&viewport, |cell, _| visible.push(cell));
        let columns: HashSet<i32> = visible.iter().map(|cell| cell.screen_column).collect();
        assert_eq!(columns, HashSet::from([0, 1]));
        assert!(visible.iter().all(|cell| cell.screen_row < 4));
        assert!(
            World::new(
                from_value(json!({"columns":1,"rows":1,"cellSize":0,"layers":[{
                "id":"map:terrain","layer":0,"kind":"gameplay"}]}))
                .unwrap()
            )
            .is_err()
        );
    }
}
