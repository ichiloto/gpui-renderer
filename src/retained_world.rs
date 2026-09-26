//! Retained, source-coordinate map data and the camera's logical-cell projection.
//! Only the visible rows are visited during a scroll. Every world cell is one
//! square field cell, whatever its text. Tile rows are graphics on the same
//! grid, independent of the owner glyphs.
use crate::retained_protocol::{
    MAX_WORLD_TILE_CELLS, Viewport, WorldCell, WorldDefinition, WorldLayerKind, WorldRow,
    WorldTileCell, WorldTileRow,
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

/// Source-state charge for one tile cell.
const TILE_CELL_BYTES: usize = 16;

/// Most characters a world cell's text may hold.
const MAX_CELL_TEXT_CHARS: usize = 8;

#[derive(Clone, Debug)]
pub struct World {
    pub definition: Arc<WorldDefinition>,
    rows: Vec<Option<Arc<WorldRow>>>,
    /// Column-sorted tile rows of each `tiles` layer.
    tiles: HashMap<String, Vec<Option<Arc<[WorldTileCell]>>>>,
    complete_rows: usize,
    owner_bytes: usize,
    tile_cells: usize,
}

/// The world cells a camera can show, and where the first one lands.
struct VisibleRange {
    columns: Range<u32>,
    rows: Range<u32>,
    pad_x: i32,
    pad_y: i32,
}

impl VisibleRange {
    fn project(&self, world_column: u32, world_row: u32) -> ProjectedCell {
        ProjectedCell {
            world_column,
            world_row,
            screen_column: self.pad_x + (world_column - self.columns.start) as i32,
            screen_row: self.pad_y + (world_row - self.rows.start) as i32,
        }
    }
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
        let tiles = definition
            .layers
            .iter()
            .filter(|layer| layer.kind == WorldLayerKind::Tiles)
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

    /// Replaces the listed rows of one tiles layer. An empty row clears it;
    /// omitted rows keep their tiles.
    pub fn replace_tile_rows(
        &mut self,
        layer_id: &str,
        updates: Vec<WorldTileRow>,
    ) -> Result<(), String> {
        let tile_count = self
            .definition
            .tileset
            .as_ref()
            .map_or(0, |tileset| tileset.tiles.len());
        let rows = self
            .tiles
            .get_mut(layer_id)
            .ok_or("worldTiles must reference a tiles layer of the world")?;
        let mut seen = HashSet::new();
        for row in &updates {
            if row.row >= self.definition.rows || !seen.insert(row.row) {
                return Err("world tile row index is out of bounds or repeated".into());
            }
            let mut columns = HashSet::new();
            for cell in &row.cells {
                if cell.column >= self.definition.columns
                    || !columns.insert(cell.column)
                    || cell.tile as usize >= tile_count
                {
                    return Err("world tile cell is out of bounds, repeated or unknown".into());
                }
            }
        }
        let next_count = updates.iter().fold(self.tile_cells, |count, row| {
            count
                - rows[row.row as usize]
                    .as_ref()
                    .map_or(0, |cells| cells.len())
                + row.cells.len()
        });
        if next_count > MAX_WORLD_TILE_CELLS {
            return Err("world exceeds 1048576 tile cells".into());
        }
        for mut row in updates {
            row.cells.sort_unstable_by_key(|cell| cell.column);
            rows[row.row as usize] = (!row.cells.is_empty()).then(|| row.cells.into());
        }
        self.tile_cells = next_count;
        Ok(())
    }

    /// The catalog tile a tiles layer paints at this cell, if any.
    #[cfg(test)]
    pub fn get_tile(&self, layer_id: &str, column: u32, row: u32) -> Option<u32> {
        let cells = self.tiles.get(layer_id)?.get(row as usize)?.as_deref()?;
        let index = cells
            .binary_search_by_key(&column, |cell| cell.column)
            .ok()?;
        Some(cells[index].tile)
    }

    /// Every tile cell of every tiles layer, in no particular order.
    pub fn visit_tiles(&self, mut visit: impl FnMut(u32, u32, u32)) {
        for rows in self.tiles.values() {
            for (row, cells) in rows.iter().enumerate() {
                for cell in cells.iter().flat_map(|cells| cells.iter()) {
                    visit(cell.column, row as u32, cell.tile);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn tile_count(&self) -> usize {
        self.tile_cells
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
        self.owner_bytes
            + self.tile_cells * TILE_CELL_BYTES
            + self.definition.layers.len() * 8192
            + self
                .definition
                .tileset
                .as_ref()
                .map_or(0, |tileset| tileset.estimated_bytes())
    }

    /// Side of one square field cell in logical pixels, before viewport scale.
    pub fn cell_size(&self) -> f32 {
        self.definition.cell_size as f32
    }

    /// Terminal text columns in one field cell.
    pub fn cell_columns(&self) -> f32 {
        self.definition.cell_columns as f32
    }

    fn get_visible_range(&self, viewport: &Viewport) -> VisibleRange {
        let first_x = viewport.world_origin.column.max(0) as u32;
        let first_y = viewport.world_origin.row.max(0) as u32;
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
        VisibleRange {
            columns: first_x..end_x.max(first_x),
            rows: first_y..end_y.max(first_y),
            pad_x: (-viewport.world_origin.column).max(0),
            pad_y: (-viewport.world_origin.row).max(0),
        }
    }

    /// Visits only the cells this camera shows; one world column is one
    /// square screen cell.
    pub fn project_visible(
        &self,
        viewport: &Viewport,
        mut visit: impl FnMut(ProjectedCell, &WorldCell),
    ) {
        let range = self.get_visible_range(viewport);
        for y in range.rows.clone() {
            let Some(row) = self.get_row(y) else { continue };
            for x in range.columns.clone() {
                // Authored rows may be ragged. Their missing trail is the
                // session background, not a new painted owner cell.
                let Some(cell) = row.cells.get(x as usize) else {
                    break;
                };
                visit(range.project(x, y), cell);
            }
        }
    }

    /// Visits the visible tile cells of one tiles layer with their catalog
    /// tile. Tiles do not depend on the owner rows' length.
    pub fn project_visible_tiles(
        &self,
        layer_id: &str,
        viewport: &Viewport,
        mut visit: impl FnMut(ProjectedCell, u32),
    ) {
        let Some(rows) = self.tiles.get(layer_id) else {
            return;
        };
        let range = self.get_visible_range(viewport);
        for y in range.rows.clone() {
            let Some(cells) = rows[y as usize].as_deref() else {
                continue;
            };
            let first = cells.partition_point(|cell| cell.column < range.columns.start);
            for cell in cells[first..]
                .iter()
                .take_while(|cell| cell.column < range.columns.end)
            {
                visit(range.project(cell.column, y), cell.tile);
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

    fn tiled(tileset: serde_json::Value) -> serde_json::Value {
        json!({"columns":4,"rows":3,"cellSize":48,"cellColumns":2,"layers":[
            {"id":"map:ground","layer":-100,"kind":"tiles"},
            {"id":"map:terrain","layer":-99,"kind":"gameplay"},
            {"id":"map:above","layer":900,"kind":"tiles"}
        ],"tileset":tileset})
    }

    fn tileset(tiles: usize) -> serde_json::Value {
        let piece = json!({"sheet":0,"x":0,"y":0,"width":48,"height":48,"left":0,"top":0});
        json!({"tileSize":48,"sheets":["tiles/home.png"],
            "tiles":vec![json!({"frames":[[piece]]}); tiles]})
    }

    fn validate(value: serde_json::Value) -> Result<(), String> {
        World::new(from_value(value).map_err(|error| error.to_string())?).map(|_| ())
    }

    #[test]
    fn tilesets_validate_their_limits_and_pieces() {
        assert!(validate(tiled(tileset(2))).is_ok());
        assert!(validate(tiled(tileset(8192))).is_ok());
        // A tiles layer needs a tileset; a glyph-only world does not.
        let mut bare = tiled(tileset(1));
        bare.as_object_mut().unwrap().remove("tileset");
        assert!(validate(bare.clone()).is_err());
        bare["layers"] = json!([{"id":"map:terrain","layer":-99,"kind":"gameplay"}]);
        assert!(validate(bare).is_ok());
        let piece = json!({"sheet":0,"x":0,"y":0,"width":1,"height":1,"left":0,"top":0});
        let with = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value = tiled(tileset(1));
            change(&mut value["tileset"]);
            validate(value)
        };
        for size in [0, 1, 3, 47, 258] {
            assert!(with(&|t| t["tileSize"] = json!(size)).is_err(), "{size}");
        }
        assert!(with(&|t| t["tileSize"] = json!(256)).is_ok());
        assert!(with(&|t| t["tileSize"] = json!(2)).is_err()); // the 48-pixel piece
        assert!(with(&|t| t["sheets"] = json!([])).is_err());
        assert!(with(&|t| t["sheets"] = json!(vec!["a.png"; 16])).is_ok());
        assert!(with(&|t| t["sheets"] = json!(vec!["a.png"; 17])).is_err());
        for path in ["", "/absolute.png"] {
            assert!(with(&|t| t["sheets"] = json!([path])).is_err(), "{path}");
        }
        assert!(validate(tiled(tileset(0))).is_err());
        assert!(validate(tiled(tileset(8193))).is_err());
        assert!(with(&|t| t["tiles"][0]["frames"] = json!([])).is_err());
        assert!(with(&|t| t["tiles"][0]["frames"] = json!(vec![[piece.clone()]; 4])).is_ok());
        assert!(with(&|t| t["tiles"][0]["frames"] = json!(vec![[piece.clone()]; 5])).is_err());
        assert!(with(&|t| t["tiles"][0]["frames"] = json!([[]])).is_err());
        assert!(with(&|t| t["tiles"][0]["frames"] = json!([vec![piece.clone(); 8]])).is_ok());
        assert!(with(&|t| t["tiles"][0]["frames"] = json!([vec![piece.clone(); 9]])).is_err());
        // Pieces are unscaled copies that must fit inside the tile.
        for (field, value) in [
            ("sheet", 1),
            ("width", 0),
            ("height", 0),
            ("left", 48),
            ("top", 48),
            ("width", 49),
            ("left", u32::MAX),
        ] {
            let mut bad = piece.clone();
            bad[field] = json!(value);
            assert!(
                with(&|t| t["tiles"][0]["frames"] = json!([[bad]])).is_err(),
                "{field}={value}"
            );
        }
        let mut corner = piece.clone();
        corner["left"] = json!(47);
        corner["top"] = json!(47);
        assert!(with(&|t| t["tiles"][0]["frames"] = json!([[corner]])).is_ok());
        let mut unknown = piece;
        unknown["scale"] = json!(2);
        assert!(with(&|t| t["tiles"][0]["frames"] = json!([[unknown]])).is_err());
        assert!(
            validate(json!({"columns":1,"rows":1,"cellSize":48,"cellColumns":2,
            "layers":[{"id":"map:ground","layer":0,"kind":"tiles"}],
            "tileset":tileset(1),"extra":true}))
            .is_err()
        );
    }

    #[test]
    fn tile_rows_replace_clear_and_stay_bounded() {
        let mut world = World::new(from_value(tiled(tileset(3))).unwrap()).unwrap();
        let empty = world.estimated_bytes();
        let row = |row: u32, cells: &[(u32, u32)]| -> WorldTileRow {
            from_value(json!({"row":row,"cells":cells.iter()
                .map(|(column, tile)| json!({"column":column,"tile":tile}))
                .collect::<Vec<_>>()}))
            .unwrap()
        };
        world
            .replace_tile_rows(
                "map:ground",
                vec![row(0, &[(3, 2), (0, 1)]), row(1, &[(1, 0)])],
            )
            .unwrap();
        assert_eq!(world.tile_count(), 3);
        assert_eq!(world.estimated_bytes(), empty + 3 * TILE_CELL_BYTES);
        assert_eq!(world.get_tile("map:ground", 0, 0), Some(1));
        assert_eq!(world.get_tile("map:ground", 3, 0), Some(2));
        assert_eq!(world.get_tile("map:ground", 1, 0), None);
        assert_eq!(world.get_tile("map:above", 0, 0), None);
        // Listed rows are replaced, an empty list clears, omitted rows persist.
        world
            .replace_tile_rows("map:ground", vec![row(0, &[(2, 0)])])
            .unwrap();
        assert_eq!(world.get_tile("map:ground", 0, 0), None);
        assert_eq!(world.get_tile("map:ground", 2, 0), Some(0));
        assert_eq!(world.get_tile("map:ground", 1, 1), Some(0));
        world
            .replace_tile_rows("map:ground", vec![row(1, &[])])
            .unwrap();
        assert_eq!(world.get_tile("map:ground", 1, 1), None);
        assert_eq!(world.tile_count(), 1);
        world
            .replace_tile_rows("map:above", vec![row(2, &[(0, 2)])])
            .unwrap();
        assert_eq!(world.tile_count(), 2);
        let before = world.clone();
        for (layer, rows) in [
            ("map:terrain", vec![row(0, &[(0, 0)])]),
            ("map:missing", vec![row(0, &[(0, 0)])]),
            ("map:ground", vec![row(3, &[(0, 0)])]),
            ("map:ground", vec![row(0, &[(4, 0)])]),
            ("map:ground", vec![row(0, &[(0, 3)])]),
            ("map:ground", vec![row(0, &[(1, 0), (1, 1)])]),
            ("map:ground", vec![row(0, &[]), row(0, &[])]),
        ] {
            assert!(world.replace_tile_rows(layer, rows).is_err(), "{layer}");
        }
        assert_eq!(world.tile_count(), before.tile_count());
        assert_eq!(world.get_tile("map:ground", 2, 0), Some(0));

        // The cap spans every tiles layer of the world.
        let wide: WorldDefinition = from_value(json!({"columns":1024,"rows":1024,
            "cellSize":48,"cellColumns":2,"layers":[
                {"id":"a","layer":0,"kind":"tiles"},{"id":"b","layer":1,"kind":"tiles"}],
            "tileset":tileset(1)}))
        .unwrap();
        let mut world = World::new(wide).unwrap();
        let full = |row: u32| -> WorldTileRow {
            from_value(json!({"row":row,"cells":(0..1024)
                .map(|column| json!({"column":column,"tile":0}))
                .collect::<Vec<_>>()}))
            .unwrap()
        };
        world
            .replace_tile_rows("a", (0..1024).map(full).collect())
            .unwrap();
        assert_eq!(world.tile_count(), MAX_WORLD_TILE_CELLS);
        assert!(
            world
                .replace_tile_rows("b", vec![row(0, &[(0, 0)])])
                .is_err()
        );
        world.replace_tile_rows("a", vec![row(0, &[])]).unwrap();
        world
            .replace_tile_rows("b", vec![row(0, &[(0, 0)])])
            .unwrap();
        assert_eq!(world.tile_count(), MAX_WORLD_TILE_CELLS - 1023);
    }

    #[test]
    fn visible_tiles_follow_the_camera_independently_of_owner_rows() {
        let mut world = World::new(from_value(tiled(tileset(2))).unwrap()).unwrap();
        // Owner rows are ragged; the tiles beyond them still project.
        world
            .replace_rows(
                (0..3)
                    .map(|index| from_value(json!({"row":index,"cells":[cell("..")]})).unwrap())
                    .collect(),
            )
            .unwrap();
        world
            .replace_tile_rows(
                "map:ground",
                vec![
                    from_value(json!({"row":1,"cells":[
                        {"column":3,"tile":1},{"column":0,"tile":0},{"column":2,"tile":1}]}))
                    .unwrap(),
                ],
            )
            .unwrap();
        // One visible cell's width past the origin column 2, plus the margin.
        let viewport: Viewport = from_value(json!({"scale":1,
            "origin":{"x":0,"y":0},"worldId":"map","tileFrame":5,
            "worldOrigin":{"column":2,"row":-1},
            "clipRect":{"x":0,"y":0,"width":48,"height":48}
        }))
        .unwrap();
        assert_eq!(viewport.tile_frame, 5);
        let mut visible = Vec::new();
        world.project_visible_tiles("map:ground", &viewport, |cell, tile| {
            visible.push((cell.world_column, cell.screen_column, cell.screen_row, tile))
        });
        assert_eq!(visible, [(2, 0, 2, 1), (3, 1, 2, 1)]);
        world.project_visible_tiles("map:above", &viewport, |_, _| panic!("no tiles"));
    }
}
