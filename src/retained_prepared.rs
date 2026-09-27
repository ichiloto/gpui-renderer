//! Prepared resources for a retained scene. A camera-only frame shares this
//! entire scene; screen art is decoded and world tiles composed only on
//! source changes.
use crate::{
    assets::AssetRoot, protocol::Grid, retained_protocol::WorldLayerKind,
    retained_state::SceneSource, retained_tileset::PreparedTileset, retained_world::World,
    state::PreparedFrame,
};
use gpui::RenderImage;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Debug)]
pub struct PreparedWorldLayer {
    pub id: String,
    pub layer: i32,
    pub kind: WorldLayerKind,
}

#[derive(Debug)]
pub struct PreparedWorld {
    pub source: Arc<World>,
    /// Layers in paint order.
    pub layers: Vec<PreparedWorldLayer>,
    /// Composed tiles, shared by every revision of the same world definition.
    pub tileset: Option<Arc<PreparedTileset>>,
    painted: PaintedCells,
}

impl PreparedWorld {
    fn prepare(
        id: &str,
        source: Arc<World>,
        assets: &AssetRoot,
        previous: Option<&PreparedWorld>,
    ) -> Self {
        let mut layers: Vec<_> = source
            .definition
            .layers
            .iter()
            .map(|layer| PreparedWorldLayer {
                id: layer.id.clone(),
                layer: layer.layer,
                kind: layer.kind,
            })
            .collect();
        layers.sort_by_key(|layer| layer.layer);
        // Row updates keep the definition; only a new put composes again.
        let tileset = match (previous, &source.definition.tileset) {
            (_, None) => None,
            (Some(old), Some(_)) if Arc::ptr_eq(&old.source.definition, &source.definition) => {
                old.tileset.clone()
            }
            (_, Some(tileset)) => {
                let (prepared, diagnostics) = PreparedTileset::prepare(tileset, assets);
                for message in diagnostics {
                    crate::protocol::diagnostic(format!("retained world {id}: {message}"));
                }
                Some(Arc::new(prepared))
            }
        };
        let mut painted = PaintedCells::new(source.definition.columns, source.definition.rows);
        if let Some(tileset) = &tileset {
            let span = source.get_tile_columns();
            source.visit_tiles(|column, row, tile| {
                if tileset.is_available(tile) {
                    for covered in column..column.saturating_add(span) {
                        painted.mark(covered, row);
                    }
                }
            });
        }
        Self {
            source,
            layers,
            tileset,
            painted,
        }
    }

    /// Whether any tiles layer paints an available tile over this cell. Such a
    /// cell does not show its glyph, even where the tile is transparent.
    pub fn has_painted_tile(&self, column: u32, row: u32) -> bool {
        self.painted.contains(column, row)
    }

    /// The composed image a catalog tile shows at this animation counter.
    pub fn get_tile_image(&self, tile: u32, tile_frame: u32) -> Option<&Arc<RenderImage>> {
        self.tileset.as_ref()?.get_frame(tile, tile_frame)
    }

    pub fn get_images(&self) -> &[Arc<RenderImage>] {
        self.tileset
            .as_ref()
            .map_or(&[], |tileset| tileset.images.as_slice())
    }
}

/// One bit per world cell.
#[derive(Debug)]
struct PaintedCells {
    columns: u32,
    bits: Vec<u64>,
}

impl PaintedCells {
    fn new(columns: u32, rows: u32) -> Self {
        Self {
            columns,
            bits: vec![0; (columns as usize * rows as usize).div_ceil(64)],
        }
    }

    fn mark(&mut self, column: u32, row: u32) {
        if column >= self.columns {
            return;
        }
        let index = row as usize * self.columns as usize + column as usize;
        self.bits[index / 64] |= 1 << (index % 64);
    }

    fn contains(&self, column: u32, row: u32) -> bool {
        if column >= self.columns {
            return false;
        }
        let index = row as usize * self.columns as usize + column as usize;
        self.bits
            .get(index / 64)
            .is_some_and(|bits| bits & (1 << (index % 64)) != 0)
    }
}

#[derive(Debug)]
pub struct PreparedScene {
    pub source: Arc<SceneSource>,
    pub screen: Arc<PreparedFrame>,
    pub worlds: BTreeMap<String, Arc<PreparedWorld>>,
    pub images: Vec<Arc<RenderImage>>,
}

impl PreparedScene {
    pub fn prepare(
        source: Arc<SceneSource>,
        frame: u64,
        grid: Grid,
        assets: &AssetRoot,
        previous: Option<&Self>,
    ) -> Result<Self, String> {
        let screen = Arc::new(PreparedFrame::prepare_v2(
            source.materialize_screen(frame)?,
            grid,
            assets,
        )?);
        let mut worlds = BTreeMap::new();
        let mut images = Vec::new();
        let mut ids = HashSet::new();
        for image in &screen.cached_images {
            if ids.insert(image.id) {
                images.push(image.clone());
            }
        }
        for (id, world) in &source.worlds {
            let old = previous.and_then(|old| old.worlds.get(id));
            let prepared = match old {
                Some(old) if Arc::ptr_eq(world, &old.source) => old.clone(),
                _ => Arc::new(PreparedWorld::prepare(
                    id,
                    world.clone(),
                    assets,
                    old.map(Arc::as_ref),
                )),
            };
            for image in prepared.get_images() {
                if ids.insert(image.id) {
                    images.push(image.clone());
                }
            }
            worlds.insert(id.clone(), prepared);
        }
        let bytes: usize = images
            .iter()
            .map(|image| image.as_bytes(0).map_or(0, |pixels| pixels.len()))
            .sum();
        // Source state has its own 64 MiB bound. Existing preparation separately
        // bounds decoded PNGs, guarded regions, and composite output at 64 MiB
        // each; their live images cannot share that single source-state budget.
        if bytes > 256 * 1024 * 1024 {
            return Err(format!(
                "retained prepared images exceed 256 MiB ({} bytes)",
                bytes
            ));
        }
        Ok(Self {
            source,
            screen,
            worlds,
            images,
        })
    }

    pub fn get_world(&self, id: &str) -> Option<&Arc<PreparedWorld>> {
        self.worlds.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retained_protocol::Operation;
    use serde_json::{from_value, json};

    fn grid() -> Grid {
        Grid {
            columns: 8,
            rows: 4,
            cell_width: 10,
            cell_height: 20,
        }
    }

    fn apply(scene: &mut SceneSource, operation: serde_json::Value) {
        scene
            .apply(from_value::<Operation>(operation).unwrap(), grid())
            .unwrap();
    }

    fn tile_row(row: u32, cells: &[(u32, u32)]) -> serde_json::Value {
        json!({"op":"worldTiles","id":"map","layerId":"map:ground","rows":[{"row":row,
            "cells":cells.iter().map(|(column, tile)| json!({"column":column,"tile":tile}))
                .collect::<Vec<_>>()}]})
    }

    #[test]
    fn painted_cells_suppress_glyphs_only_under_available_tiles() {
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]))
            .save(directory.path().join("ground.png"))
            .unwrap();
        let assets = AssetRoot::new(directory.path()).unwrap();
        let piece = |sheet: u32| json!([[{"sheet":sheet,"x":0,"y":0,"width":2,"height":2,"left":0,"top":0}]]);
        let mut scene = SceneSource::default();
        apply(
            &mut scene,
            json!({"op":"put","kind":"world","id":"map","value":{
            "columns":3,"rows":2,"cellWidth":24,"cellHeight":48,"layers":[
                {"id":"map:above","layer":900,"kind":"tiles"},
                {"id":"map:ground","layer":-100,"kind":"tiles"},
                {"id":"map:terrain","layer":-99,"kind":"gameplay"}],
            "tileset":{"tileSize":2,"sheets":["ground.png","missing.png"],"tiles":[
                {"frames":piece(0)},{"frames":piece(1)}]}}}),
        );
        apply(&mut scene, tile_row(0, &[(0, 0), (2, 1)]));
        apply(
            &mut scene,
            json!({"op":"worldTiles","id":"map","layerId":"map:above","rows":[
                {"row":1,"cells":[{"column":2,"tile":0}]}]}),
        );
        let first = PreparedScene::prepare(Arc::new(scene.clone()), 1, grid(), &assets, None)
            .expect("a missing sheet never rejects the scene");
        let world = first.get_world("map").unwrap();
        let painted = |world: &PreparedWorld| {
            (0..2)
                .flat_map(|row| (0..3).map(move |column| (column, row)))
                .filter(|(column, row)| world.has_painted_tile(*column, *row))
                .collect::<Vec<_>>()
        };
        // A tile covers its cell and the next across (24 x 48 cells, square
        // tiles), within the world. Tile 1 uses the missing sheet, so its cell
        // keeps the glyph.
        assert_eq!(painted(world), [(0, 0), (1, 0), (2, 1)]);
        assert!(!world.has_painted_tile(3, 0));
        assert!(world.get_tile_image(1, 0).is_none());
        let composed = world.get_tile_image(0, 0).unwrap();
        assert!(first.images.iter().any(|image| image.id == composed.id));
        let kinds: Vec<_> = world.layers.iter().map(|layer| layer.kind).collect();
        assert_eq!(
            kinds,
            [
                WorldLayerKind::Tiles,
                WorldLayerKind::Gameplay,
                WorldLayerKind::Tiles
            ]
        );

        // A row update keeps the composed tiles and refreshes the bookkeeping.
        apply(&mut scene, tile_row(0, &[(1, 0)]));
        let second =
            PreparedScene::prepare(Arc::new(scene.clone()), 2, grid(), &assets, Some(&first))
                .unwrap();
        let updated = second.get_world("map").unwrap();
        assert!(Arc::ptr_eq(
            world.tileset.as_ref().unwrap(),
            updated.tileset.as_ref().unwrap()
        ));
        assert_eq!(painted(updated), [(1, 0), (2, 0), (2, 1)]);

        // Putting the world again clears its tiles and composes afresh.
        apply(
            &mut scene,
            json!({"op":"put","kind":"world","id":"map","value":{
            "columns":3,"rows":2,"cellWidth":24,"cellHeight":48,"layers":[
                {"id":"map:ground","layer":-100,"kind":"tiles"},
                {"id":"map:terrain","layer":-99,"kind":"gameplay"}],
            "tileset":{"tileSize":2,"sheets":["ground.png"],"tiles":[{"frames":piece(0)}]}}}),
        );
        let third =
            PreparedScene::prepare(Arc::new(scene), 3, grid(), &assets, Some(&second)).unwrap();
        let replaced = third.get_world("map").unwrap();
        assert!(painted(replaced).is_empty());
        assert!(!Arc::ptr_eq(
            updated.tileset.as_ref().unwrap(),
            replaced.tileset.as_ref().unwrap()
        ));
    }
}
