//! Map worlds for authoring tools. An editor sends the same world operations
//! the game's `PresentationWorld` produces and paints its tiles layers with
//! the renderer's own composition, sampling and draw bands, so a map looks in
//! the editor as it does in the game. Glyphs and editing overlays stay the
//! tool's own; [`MapWorld::has_covering_tile`] tells it which glyphs the
//! graphical field hides.
use crate::{
    assets::AssetRoot,
    display_cache::DisplayRasterCache,
    protocol::{ViewportPoint, ViewportRect},
    retained_prepared::PreparedWorld,
    retained_protocol::{
        EntityKind, Operation, Viewport, WorldDefinition, WorldLayerKind, WorldOrigin, validate_id,
    },
    retained_world::World,
    viewport::ViewportTransform,
};
use gpui::{IntoElement, Window};
use serde_json::Value;
use std::{cell::RefCell, collections::HashSet, path::Path, rc::Rc, sync::Arc};

/// The project's asset root, where tileset sheets are read from. Clones share
/// decoded sheets and composed tiles.
#[derive(Clone)]
pub struct MapAssets(AssetRoot);

impl MapAssets {
    pub fn new(root: &Path) -> Result<Self, String> {
        AssetRoot::new(root).map(Self)
    }
}

/// What a world layer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapLayerKind {
    /// Glyphs of a gameplay layer.
    Gameplay,
    /// Glyphs of a decoration layer.
    Decoration,
    /// Tiles from the world's tileset.
    Tiles,
}

/// One world layer, in paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapWorldLayer<'a> {
    pub id: &'a str,
    /// The draw band: higher layers paint over lower ones.
    pub layer: i32,
    pub kind: MapLayerKind,
}

/// A map world prepared for painting.
#[derive(Clone)]
pub struct MapWorld {
    id: String,
    prepared: Arc<PreparedWorld>,
}

impl MapWorld {
    /// Builds a world from a world `put` followed by its `worldRows` and
    /// `worldTiles`, validated as the renderer validates them.
    pub fn from_operations(operations: Vec<Value>, assets: &MapAssets) -> Result<Self, String> {
        Self::apply_operations(None, operations, assets)
    }

    /// Applies further operations to this world. Row and tile updates keep
    /// its composed tiles; a new world `put` composes them again.
    pub fn update(&self, operations: Vec<Value>, assets: &MapAssets) -> Result<Self, String> {
        Self::apply_operations(Some(self), operations, assets)
    }

    fn apply_operations(
        previous: Option<&Self>,
        operations: Vec<Value>,
        assets: &MapAssets,
    ) -> Result<Self, String> {
        let mut id = previous.map(|world| world.id.clone());
        let mut world = previous.map(|world| World::clone(&world.prepared.source));
        for value in operations {
            let operation =
                serde_json::from_value::<Operation>(value).map_err(|error| error.to_string())?;
            match operation {
                Operation::Put {
                    kind: EntityKind::World,
                    id: put,
                    value,
                } => {
                    validate_id(&put)?;
                    let definition = serde_json::from_value::<WorldDefinition>(value)
                        .map_err(|error| error.to_string())?;
                    world = Some(World::new(definition)?);
                    id = Some(put);
                }
                Operation::WorldRows { id: target, rows } => {
                    Self::find_target(&mut world, id.as_deref(), &target)?.replace_rows(rows)?
                }
                Operation::WorldTiles {
                    id: target,
                    layer_id,
                    rows,
                } => Self::find_target(&mut world, id.as_deref(), &target)?
                    .replace_tile_rows(&layer_id, rows)?,
                _ => return Err("a map world takes only a world put and its rows".into()),
            }
        }
        let (Some(id), Some(world)) = (id, world) else {
            return Err("a map world starts with a world put".into());
        };
        world.validate_complete()?;
        let prepared = PreparedWorld::prepare(
            &id,
            Arc::new(world),
            &assets.0,
            previous.map(|world| world.prepared.as_ref()),
        );
        Ok(Self {
            id,
            prepared: Arc::new(prepared),
        })
    }

    fn find_target<'a>(
        world: &'a mut Option<World>,
        id: Option<&str>,
        target: &str,
    ) -> Result<&'a mut World, String> {
        match world {
            Some(world) if id == Some(target) => Ok(world),
            _ => Err(format!("{target} is not this map world")),
        }
    }

    /// Columns and rows.
    pub fn get_size(&self) -> (u32, u32) {
        let definition = &self.prepared.source.definition;
        (definition.columns, definition.rows)
    }

    /// One cell's width and height in logical pixels, before camera scale.
    pub fn get_cell_size(&self) -> (f32, f32) {
        self.prepared.source.cell_size()
    }

    pub fn get_layers(&self) -> impl Iterator<Item = MapWorldLayer<'_>> {
        self.prepared.layers.iter().map(|layer| MapWorldLayer {
            id: &layer.id,
            layer: layer.layer,
            kind: match layer.kind {
                WorldLayerKind::Gameplay => MapLayerKind::Gameplay,
                WorldLayerKind::Decoration => MapLayerKind::Decoration,
                WorldLayerKind::Tiles => MapLayerKind::Tiles,
            },
        })
    }

    /// Whether the graphical field hides the glyph its owner layer shows at
    /// this cell, because a usable tile covers it.
    pub fn has_covering_tile(&self, column: u32, row: u32, owner_layer_id: &str) -> bool {
        self.prepared.has_covering_tile(column, row, owner_layer_id)
    }

    /// Paints one tiles layer's visible tiles through `camera`, filling its
    /// parent. Nothing for an unknown or glyph layer.
    pub fn paint_tiles(
        &self,
        layer_id: &str,
        camera: MapCamera,
        samples: &MapTileSamples,
    ) -> Option<impl IntoElement + use<>> {
        let index = self
            .prepared
            .layers
            .iter()
            .position(|layer| layer.id == layer_id && layer.kind == WorldLayerKind::Tiles)?;
        Some(crate::retained_paint::tile_element(
            self.prepared.clone(),
            index,
            ViewportTransform::fit(camera.width, camera.height, camera.width, camera.height),
            camera.get_viewport(self.get_cell_size()),
            samples.0.clone(),
        ))
    }
}

/// Where a world is drawn inside its element, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapCamera {
    /// Where the world's top-left corner lands, relative to the element.
    pub left: f32,
    pub top: f32,
    /// Logical pixels per world pixel.
    pub scale: f32,
    /// The element's size; nothing paints outside it.
    pub width: f32,
    pub height: f32,
    /// Animation counter; a tile with N frames shows frame `tile_frame % N`.
    pub tile_frame: u32,
}

impl MapCamera {
    /// The renderer viewport showing the same cells. Its world origin is the
    /// cell under the element's corner, negative while the map starts inside
    /// the element, and its origin is where that cell lands.
    fn get_viewport(&self, (cell_width, cell_height): (f32, f32)) -> Viewport {
        let first = |offset: f32, pitch: f32| {
            if pitch > 0.0 && offset.is_finite() {
                (-offset / pitch).floor() as i32
            } else {
                0
            }
        };
        let (pitch_x, pitch_y) = (cell_width * self.scale, cell_height * self.scale);
        let (column, row) = (first(self.left, pitch_x), first(self.top, pitch_y));
        Viewport {
            scale: self.scale,
            origin: ViewportPoint {
                x: self.left + column as f32 * pitch_x,
                y: self.top + row as f32 * pitch_y,
            },
            world_origin: WorldOrigin { column, row },
            clip_rect: ViewportRect {
                x: 0.0,
                y: 0.0,
                width: self.width,
                height: self.height,
            },
            world_id: None,
            text_layer_ids: Vec::new(),
            sprite_ids: Vec::new(),
            tile_frame: self.tile_frame,
            follow: None,
        }
    }
}

/// Device-resolution tile samples shared by every map world a window paints.
#[derive(Clone, Default)]
pub struct MapTileSamples(Rc<RefCell<DisplayRasterCache>>);

impl MapTileSamples {
    /// Releases samples of tiles none of `worlds` shows any more. Call once
    /// per frame, before painting them.
    pub fn begin_frame<'a>(
        &self,
        worlds: impl IntoIterator<Item = &'a MapWorld>,
        window: &mut Window,
    ) {
        let regions: HashSet<_> = worlds
            .into_iter()
            .flat_map(|world| world.prepared.get_images())
            .map(|image| image.id)
            .collect();
        for retired in self.0.borrow_mut().begin_frame(&regions, &HashSet::new()) {
            if let Err(error) = window.drop_image(retired) {
                crate::protocol::diagnostic(format!("cannot retire map tile sample: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn world_put(tileset: bool) -> Value {
        let piece = json!([[{"sheet":0,"x":0,"y":0,"width":2,"height":2,"left":0,"top":0}]]);
        let mut value = json!({"columns":3,"rows":2,"cellWidth":24,"cellHeight":48,"layers":[
            {"id":"map:terrain","layer":-99,"kind":"gameplay"},
            {"id":"map:detail","layer":-98,"kind":"decoration"}]});
        if tileset {
            value["layers"].as_array_mut().unwrap().insert(
                0,
                json!({"id":"tiles:floor","layer":-100,"kind":"tiles","coversLayerId":"map:terrain"}),
            );
            value["tileset"] =
                json!({"tileSize":2,"sheets":["floor.png"],"tiles":[{"frames":piece}]});
        }
        json!({"op":"put","kind":"world","id":"map","value":value})
    }

    fn rows() -> Value {
        let cell = json!({"glyph":".","ownerLayerId":"map:terrain"});
        json!({"op":"worldRows","id":"map","rows":[
            {"row":0,"cells":[cell, cell, cell]},{"row":1,"cells":[cell, cell, cell]}]})
    }

    fn tiles(row: u32, columns: &[u32]) -> Value {
        json!({"op":"worldTiles","id":"map","layerId":"tiles:floor","rows":[{"row":row,
            "cells":columns.iter().map(|column| json!({"column":column,"tile":0})).collect::<Vec<_>>()}]})
    }

    fn assets() -> (tempfile::TempDir, MapAssets) {
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(directory.path().join("floor.png"))
            .unwrap();
        let assets = MapAssets::new(directory.path()).unwrap();
        (directory, assets)
    }

    fn covered(world: &MapWorld) -> Vec<(u32, u32)> {
        (0..2)
            .flat_map(|row| (0..3).map(move |column| (column, row)))
            .filter(|&(column, row)| world.has_covering_tile(column, row, "map:terrain"))
            .collect()
    }

    #[test]
    fn builds_a_world_from_the_game_operations_and_keeps_its_tiles_across_updates() {
        let (_directory, assets) = assets();
        let world =
            MapWorld::from_operations(vec![world_put(true), rows(), tiles(0, &[0, 2])], &assets)
                .unwrap();
        assert_eq!(world.get_size(), (3, 2));
        assert_eq!(world.get_cell_size(), (24.0, 48.0));
        assert_eq!(
            world.get_layers().collect::<Vec<_>>(),
            [
                MapWorldLayer {
                    id: "tiles:floor",
                    layer: -100,
                    kind: MapLayerKind::Tiles
                },
                MapWorldLayer {
                    id: "map:terrain",
                    layer: -99,
                    kind: MapLayerKind::Gameplay
                },
                MapWorldLayer {
                    id: "map:detail",
                    layer: -98,
                    kind: MapLayerKind::Decoration
                },
            ]
        );
        assert_eq!(covered(&world), [(0, 0), (2, 0)]);

        // A tile edit is a row update: the composed tileset is shared.
        let edited = world
            .update(vec![tiles(0, &[]), tiles(1, &[1])], &assets)
            .unwrap();
        assert_eq!(covered(&edited), [(1, 1)]);
        assert!(Arc::ptr_eq(
            world.prepared.tileset.as_ref().unwrap(),
            edited.prepared.tileset.as_ref().unwrap()
        ));
        // A new put composes again; a world without a tileset covers nothing.
        let plain = edited
            .update(vec![world_put(false), rows()], &assets)
            .unwrap();
        assert!(plain.prepared.tileset.is_none());
        assert_eq!(covered(&plain), []);
        assert!(
            plain
                .paint_tiles("map:terrain", camera(0.0, 0.0), &MapTileSamples::default())
                .is_none()
        );
        assert!(
            world
                .paint_tiles("tiles:floor", camera(0.0, 0.0), &MapTileSamples::default())
                .is_some()
        );
    }

    #[test]
    fn refuses_anything_but_a_complete_world() {
        let (_directory, assets) = assets();
        let refused =
            |operations: Vec<Value>| MapWorld::from_operations(operations, &assets).is_err();
        assert!(refused(vec![]));
        assert!(refused(vec![rows()]));
        // Every owner row must arrive.
        assert!(refused(vec![world_put(true)]));
        assert!(refused(vec![
            world_put(true),
            rows(),
            json!({"op":"worldRows","id":"other","rows":[]})
        ]));
        assert!(refused(vec![
            world_put(true),
            rows(),
            json!({"op":"remove","kind":"world","id":"map"})
        ]));
        assert!(refused(vec![world_put(true), rows(), tiles(2, &[0])]));
        let world = MapWorld::from_operations(vec![world_put(true), rows()], &assets).unwrap();
        assert!(world.update(vec![tiles(0, &[3])], &assets).is_err());
    }

    fn camera(left: f32, top: f32) -> MapCamera {
        MapCamera {
            left,
            top,
            scale: 0.5,
            width: 100.0,
            height: 80.0,
            tile_frame: 0,
        }
    }

    #[test]
    fn a_camera_lands_each_cell_where_the_editor_draws_it() {
        let (_directory, assets) = assets();
        let world = MapWorld::from_operations(vec![world_put(true), rows()], &assets).unwrap();
        let cell = world.get_cell_size();
        // Each world cell lands at left + column * pitch, whether the map
        // starts inside the element or has been scrolled past its corner.
        for (left, top) in [(30.0, 10.0), (0.0, 0.0), (-20.0, -30.0), (-30.0, -20.0)] {
            let camera = camera(left, top);
            let viewport = camera.get_viewport(cell);
            let mut seen = 0;
            world
                .prepared
                .source
                .project_visible(&viewport, |projected, _| {
                    let bounds = crate::retained_paint::cell_bounds(
                        projected,
                        cell,
                        (0.0, 0.0, cell.0),
                        ViewportTransform::fit(100.0, 80.0, 100.0, 80.0),
                        &viewport,
                    );
                    assert_eq!(
                        (bounds.left, bounds.top, bounds.width, bounds.height),
                        (
                            left + projected.world_column as f32 * 12.0,
                            top + projected.world_row as f32 * 24.0,
                            12.0,
                            24.0
                        )
                    );
                    seen += 1;
                });
            assert!(seen > 0, "{left},{top}");
        }
    }
}
