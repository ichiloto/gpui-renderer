//! Prepared resources for a retained scene. A camera-only frame shares this
//! entire scene; screen art is decoded and world tiles composed only on
//! source changes.
use crate::{
    assets::AssetRoot,
    protocol::{Grid, SourceRect},
    retained_protocol::WorldLayerKind,
    retained_state::SceneSource,
    retained_tileset::PreparedTileset,
    retained_world::World,
    state::PreparedFrame,
};
use gpui::{ImageId, RenderImage};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

const MAX_PREPARED_SCENE_BYTES: usize = 256 * 1024 * 1024;

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
    turned_sprites: BTreeMap<String, PreparedTurnedSprite>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SpriteTurnSource {
    image: ImageId,
    source_rect: Option<SourceRect>,
    quarter_turns: u8,
}

#[derive(Debug)]
struct PreparedTurnedSprite {
    source: SpriteTurnSource,
    image: Arc<RenderImage>,
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
        let mut live_image_ids = HashSet::new();
        let mut live_image_bytes = 0usize;
        for image in &screen.cached_images {
            if live_image_ids.insert(image.id) {
                live_image_bytes += image.as_bytes(0).map_or(0, |pixels| pixels.len());
            }
        }
        // GPUI 0.2.2 does not transform polychrome images. Prepare only the
        // retained sprites that request rotation, reusing identical pixels
        // across scene revisions. Crop precedes rotation; display bounds stay
        // with the authored sprite, so its bottom-center anchor is unchanged.
        let mut prepared_turns: HashMap<SpriteTurnSource, Arc<RenderImage>> = previous
            .into_iter()
            .flat_map(|old| old.turned_sprites.values())
            .map(|old| (old.source, old.image.clone()))
            .collect();
        let mut turned_sprites = BTreeMap::new();
        for sprite in &screen.sprites {
            let quarter_turns = source.get_sprite_quarter_turns(&sprite.sprite.id);
            if quarter_turns == 0 {
                continue;
            }
            let key = SpriteTurnSource {
                image: sprite.image.id,
                source_rect: sprite.sprite.source_rect,
                quarter_turns,
            };
            // Check the exact output allocation before constructing it. A
            // malformed update with many distinct turns must not transiently
            // allocate hundreds of MiB before the final scene-budget check.
            let source_size = sprite.image.size(0);
            let rect = key.source_rect.unwrap_or(SourceRect {
                x: 0,
                y: 0,
                width: source_size.width.0 as u32,
                height: source_size.height.0 as u32,
            });
            let expected_bytes = rect.width as usize * rect.height as usize * 4;
            if !prepared_turns.contains_key(&key)
                && live_image_bytes.saturating_add(expected_bytes) > MAX_PREPARED_SCENE_BYTES
            {
                return Err("retained prepared images exceed 256 MiB".into());
            }
            let image = match prepared_turns.get(&key) {
                Some(image) => image.clone(),
                None => {
                    let image =
                        turn_sprite_image(&sprite.image, sprite.sprite.source_rect, quarter_turns)?;
                    prepared_turns.insert(key, image.clone());
                    image
                }
            };
            if live_image_ids.insert(image.id) {
                live_image_bytes += image.as_bytes(0).map_or(0, |pixels| pixels.len());
                if live_image_bytes > MAX_PREPARED_SCENE_BYTES {
                    return Err("retained prepared images exceed 256 MiB".into());
                }
            }
            turned_sprites.insert(
                sprite.sprite.id.clone(),
                PreparedTurnedSprite { source: key, image },
            );
        }
        let mut worlds = BTreeMap::new();
        let mut images = Vec::new();
        let mut ids = HashSet::new();
        for image in &screen.cached_images {
            if ids.insert(image.id) {
                images.push(image.clone());
            }
        }
        for turned in turned_sprites.values() {
            if ids.insert(turned.image.id) {
                images.push(turned.image.clone());
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
        if bytes > MAX_PREPARED_SCENE_BYTES {
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
            turned_sprites,
        })
    }

    pub fn get_turned_sprite_image(&self, id: &str) -> Option<&Arc<RenderImage>> {
        self.turned_sprites.get(id).map(|sprite| &sprite.image)
    }

    pub fn get_world(&self, id: &str) -> Option<&Arc<PreparedWorld>> {
        self.worlds.get(id)
    }
}

/// Select source pixels before the clockwise turn. RenderImage stores BGRA,
/// so copying whole pixels keeps straight alpha and color channels untouched.
/// A non-square crop swaps pixel dimensions for odd turns, then fills the
/// sprite's unchanged authored destination box just as a sourceRect crop does.
fn turn_sprite_image(
    source: &Arc<RenderImage>,
    source_rect: Option<SourceRect>,
    quarter_turns: u8,
) -> Result<Arc<RenderImage>, String> {
    if !(1..=3).contains(&quarter_turns) {
        return Err("sprite quarterTurns must be 1, 2, or 3 when preparing a turn".into());
    }
    let size = source.size(0);
    let source_width = size.width.0 as u32;
    let source_height = size.height.0 as u32;
    let rect = source_rect.unwrap_or(SourceRect {
        x: 0,
        y: 0,
        width: source_width,
        height: source_height,
    });
    rect.validate_image(source_width, source_height)?;
    let (width, height) = if quarter_turns.is_multiple_of(2) {
        (rect.width, rect.height)
    } else {
        (rect.height, rect.width)
    };
    let source_bytes = source.as_bytes(0).ok_or("decoded sprite has no pixels")?;
    let mut pixels = vec![0; width as usize * height as usize * 4];
    for sy in 0..rect.height {
        for sx in 0..rect.width {
            let (dx, dy) = match quarter_turns {
                1 => (rect.height - 1 - sy, sx),
                2 => (rect.width - 1 - sx, rect.height - 1 - sy),
                3 => (sy, rect.width - 1 - sx),
                _ => unreachable!(),
            };
            let from =
                ((rect.y + sy) as usize * source_width as usize + (rect.x + sx) as usize) * 4;
            let to = (dy as usize * width as usize + dx as usize) * 4;
            pixels[to..to + 4].copy_from_slice(&source_bytes[from..from + 4]);
        }
    }
    let pixels = image::RgbaImage::from_raw(width, height, pixels)
        .ok_or("prepared sprite turn has invalid pixel dimensions")?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retained_protocol::Operation;
    use serde_json::{from_value, json};

    fn pixel(image: &RenderImage, x: u32, y: u32) -> [u8; 4] {
        let width = image.size(0).width.0 as u32;
        let offset = (y as usize * width as usize + x as usize) * 4;
        image.as_bytes(0).unwrap()[offset..offset + 4]
            .try_into()
            .unwrap()
    }

    #[test]
    fn sprite_quarter_turns_crop_first_rotate_clockwise_and_copy_alpha() {
        // The selected 2x3 block is 1 2 / 3 4 / 5 6. Bright neighbors
        // must never leak into the derived image, including its alpha edge.
        let source = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_fn(4, 3, |x, y| {
                let value = if (1..=2).contains(&x) {
                    (y * 2 + x) as u8
                } else {
                    200
                };
                image::Rgba([value, value + 10, value + 20, value + 30])
            }),
        )]));
        let original = source.as_bytes(0).unwrap().to_vec();
        let selected = Some(SourceRect {
            x: 1,
            y: 0,
            width: 2,
            height: 3,
        });
        for (turns, width, height, expected) in [
            (1, 3, 2, vec![5, 3, 1, 6, 4, 2]),
            (2, 2, 3, vec![6, 5, 4, 3, 2, 1]),
            (3, 3, 2, vec![2, 4, 6, 1, 3, 5]),
        ] {
            let turned = turn_sprite_image(&source, selected, turns).unwrap();
            assert_eq!(
                (turned.size(0).width.0, turned.size(0).height.0),
                (width as i32, height as i32)
            );
            let mut actual = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    actual.push(pixel(&turned, x, y));
                }
            }
            assert_eq!(
                actual,
                expected
                    .into_iter()
                    .map(|value| [value, value + 10, value + 20, value + 30])
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(source.as_bytes(0).unwrap(), original);
        assert!(turn_sprite_image(&source, selected, 0).is_err());
        assert!(turn_sprite_image(&source, selected, 4).is_err());
    }

    #[test]
    fn retained_sprite_turns_reuse_images_and_retire_on_replacement() {
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_fn(4, 3, |x, y| image::Rgba([x as u8, y as u8, 90, 200]))
            .save(directory.path().join("arrow.png"))
            .unwrap();
        let assets = AssetRoot::new(directory.path()).unwrap();
        let mut scene = SceneSource::default();
        let sprite = |turns: Option<u8>| {
            let mut value = json!({"id":"arrow","order":0,"asset":"arrow.png",
                "x":1,"y":1,"width":24,"height":24,"anchor":"bottom_center",
                "layer":100,"sourceRect":{"x":1,"y":0,"width":2,"height":3}});
            if let Some(turns) = turns {
                value["quarterTurns"] = json!(turns);
            }
            json!({"op":"put","kind":"sprite","id":"arrow","value":value})
        };
        apply(&mut scene, sprite(Some(1)));
        let first =
            PreparedScene::prepare(Arc::new(scene.clone()), 1, grid(), &assets, None).unwrap();
        let first_image = first.get_turned_sprite_image("arrow").unwrap();
        assert!(first.images.iter().any(|image| image.id == first_image.id));
        assert_eq!(first.screen.sprites[0].sprite.source_rect.unwrap().width, 2);
        // Pixel dimensions can swap on odd turns, but authored destination
        // bounds remain the source of the bottom-center ground contact.
        for scale in [1.0, 0.75, 0.5] {
            let transform = crate::viewport::ViewportTransform::fit(
                80.0,
                80.0,
                80.0 * scale + 20.0,
                80.0 * scale,
            );
            let bounds = crate::renderer::sprite_bounds(
                &first.screen.sprites[0].sprite,
                0,
                (10.0, 20.0),
                transform,
            );
            assert_eq!(
                (
                    bounds.left + bounds.width / 2.0 + transform.offset_x,
                    bounds.top + bounds.height + transform.offset_y
                ),
                (15.0 * scale + 10.0, 40.0 * scale)
            );
        }

        // Unrelated screen source changes retain the same prepared orientation.
        apply(
            &mut scene,
            json!({"op":"put","kind":"text","id":"hud","value":{
            "id":"hud","order":0,"layer":200,"runs":[]}}),
        );
        let second =
            PreparedScene::prepare(Arc::new(scene.clone()), 2, grid(), &assets, Some(&first))
                .unwrap();
        assert!(Arc::ptr_eq(
            first_image,
            second.get_turned_sprite_image("arrow").unwrap()
        ));

        apply(&mut scene, sprite(Some(2)));
        let third =
            PreparedScene::prepare(Arc::new(scene.clone()), 3, grid(), &assets, Some(&second))
                .unwrap();
        let third_image = third.get_turned_sprite_image("arrow").unwrap();
        assert_ne!(first_image.id, third_image.id);
        assert!(!third.images.iter().any(|image| image.id == first_image.id));

        // Omission retains the legacy sourceRect paint path. No turned atlas
        // image survives in the next scene, so the renderer can retire it.
        apply(&mut scene, sprite(None));
        let fourth =
            PreparedScene::prepare(Arc::new(scene), 4, grid(), &assets, Some(&third)).unwrap();
        assert!(fourth.get_turned_sprite_image("arrow").is_none());
        assert!(!fourth.images.iter().any(|image| image.id == third_image.id));
    }

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
