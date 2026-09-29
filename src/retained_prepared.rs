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
    collections::{BTreeMap, HashMap, HashSet},
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
    painted: PaintedTiles,
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
        let mut painted = PaintedTiles::new(source.definition.columns, source.definition.rows);
        if let Some(tileset) = &tileset {
            let covers: HashMap<&str, &str> = source
                .definition
                .layers
                .iter()
                .filter_map(|layer| Some((layer.id.as_str(), layer.covers_layer_id.as_deref()?)))
                .collect();
            source.visit_tiles(|layer_id, column, row, tile| {
                if tileset.is_available(tile) {
                    painted.mark(covers.get(layer_id).copied(), column, row);
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

    /// Whether a tiles layer places an available tile at this cell that
    /// covers the glyph of the gameplay layer owning it: a layer that covers
    /// that gameplay layer, or one that covers no particular layer. Such a
    /// cell does not show its glyph, even where the tile is transparent; a
    /// tile's overhang into the cells beside it does not hide theirs.
    pub fn has_covering_tile(&self, column: u32, row: u32, owner_layer_id: &str) -> bool {
        self.painted.covers(column, row, owner_layer_id)
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

/// The cells tiles paint: those of tiles layers that cover no particular
/// layer, and those of each covered gameplay layer.
#[derive(Debug)]
struct PaintedTiles {
    columns: u32,
    rows: u32,
    uncovered: PaintedCells,
    covered: HashMap<String, PaintedCells>,
}

impl PaintedTiles {
    fn new(columns: u32, rows: u32) -> Self {
        Self {
            columns,
            rows,
            uncovered: PaintedCells::new(columns, rows),
            covered: HashMap::new(),
        }
    }

    fn mark(&mut self, covers: Option<&str>, column: u32, row: u32) {
        let (columns, rows) = (self.columns, self.rows);
        match covers {
            None => &mut self.uncovered,
            Some(layer_id) => self
                .covered
                .entry(layer_id.to_owned())
                .or_insert_with(|| PaintedCells::new(columns, rows)),
        }
        .mark(column, row);
    }

    fn covers(&self, column: u32, row: u32, owner_layer_id: &str) -> bool {
        self.uncovered.contains(column, row)
            || self
                .covered
                .get(owner_layer_id)
                .is_some_and(|cells| cells.contains(column, row))
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
                .filter(|(column, row)| world.has_covering_tile(*column, *row, "map:terrain"))
                .collect::<Vec<_>>()
        };
        // These tiles layers cover no particular layer, so a tile hides the
        // glyph of its own cell whatever layer owns it. Tile 1 uses the
        // missing sheet, so its cell keeps the glyph.
        assert_eq!(painted(world), [(0, 0), (2, 1)]);
        assert!(!world.has_covering_tile(3, 0, "map:terrain"));
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
        assert_eq!(painted(updated), [(1, 0), (2, 1)]);

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

    fn covered_world(layers: serde_json::Value) -> serde_json::Value {
        json!({"op":"put","kind":"world","id":"map","value":{
            "columns":4,"rows":1,"cellWidth":48,"cellHeight":48,"layers":layers,
            "tileset":{"tileSize":2,"sheets":["ground.png"],"tiles":[
                {"frames":[[{"sheet":0,"x":0,"y":0,"width":2,"height":2,"left":0,"top":0}]]}]}}})
    }

    #[test]
    fn tiles_cover_only_the_glyphs_of_the_gameplay_layer_they_belong_to() {
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]))
            .save(directory.path().join("ground.png"))
            .unwrap();
        let assets = AssetRoot::new(directory.path()).unwrap();
        let mut scene = SceneSource::default();
        apply(
            &mut scene,
            covered_world(json!([
                {"id":"map:buildings","layer":-97,"kind":"gameplay"},
                {"id":"map:fixtures","layer":-94,"kind":"gameplay"},
                {"id":"tiles:floor","layer":-99,"kind":"tiles","coversLayerId":"map:buildings"},
                {"id":"tiles:furniture","layer":-96,"kind":"tiles","coversLayerId":"map:fixtures"},
                {"id":"tiles:furniture:above","layer":904,"kind":"tiles","coversLayerId":"map:fixtures"},
                {"id":"tiles:plain","layer":-95,"kind":"tiles"}])),
        );
        let tiles = |layer: &str, columns: &[u32]| {
            json!({"op":"worldTiles","id":"map","layerId":layer,"rows":[{"row":0,
                "cells":columns.iter().map(|column| json!({"column":column,"tile":0}))
                    .collect::<Vec<_>>()}]})
        };
        apply(&mut scene, tiles("tiles:floor", &[0, 1, 2, 3]));
        apply(&mut scene, tiles("tiles:furniture", &[1]));
        apply(&mut scene, tiles("tiles:furniture:above", &[2]));
        apply(&mut scene, tiles("tiles:plain", &[3]));
        let prepared = PreparedScene::prepare(Arc::new(scene), 1, grid(), &assets, None).unwrap();
        let world = prepared.get_world("map").unwrap();
        let covered = |owner: &str| {
            (0..4)
                .filter(|column| world.has_covering_tile(*column, 0, owner))
                .collect::<Vec<_>>()
        };
        // The floor belongs to buildings: a fixtures glyph over it keeps
        // showing. Furniture tiles in either band hide the fixtures glyph
        // under them, and a tiles layer covering no particular layer still
        // hides every glyph it paints.
        assert_eq!(covered("map:fixtures"), [1, 2, 3]);
        assert_eq!(covered("map:buildings"), [0, 1, 2, 3]);
        assert!(!world.has_covering_tile(4, 0, "map:fixtures"));
    }

    #[test]
    fn a_tiles_layer_covers_only_a_gameplay_layer_of_its_world() {
        let layers = |covers: serde_json::Value, kind: &str| {
            json!([{"id":"map:terrain","layer":-99,"kind":"gameplay"},
                {"id":"map:detail","layer":-98,"kind":"decoration"},
                {"id":"tiles:floor","layer":-99,"kind":"tiles"},
                {"id":"tiles:rug","layer":-98,"kind":kind,"coversLayerId":covers}])
        };
        let put = |layers: serde_json::Value| {
            SceneSource::default().apply(
                from_value::<Operation>(covered_world(layers)).unwrap(),
                grid(),
            )
        };
        assert!(put(layers(json!("map:terrain"), "tiles")).is_ok());
        for rejected in [
            layers(json!("map:detail"), "tiles"),
            layers(json!("tiles:floor"), "tiles"),
            layers(json!("map:roof"), "tiles"),
            layers(json!("map:terrain"), "gameplay"),
        ] {
            assert_eq!(
                put(rejected).unwrap_err(),
                "coversLayerId belongs on a tiles layer and names a gameplay layer of the world"
            );
        }
    }
}
