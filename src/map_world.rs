//! Map worlds for authoring tools. An editor sends the same world operations
//! the game's `PresentationWorld` produces and paints all its layers with
//! the renderer's own composition, sampling and draw bands, so a map looks in
//! the editor as it does in the game. Editing overlays remain the tool's own;
//! [`MapWorld::paint_world_layer`] paints tiles, shadows and composed glyphs.
use crate::{
    assets::AssetRoot,
    display_cache::DisplayRasterCache,
    protocol::{ViewportPoint, ViewportRect},
    retained_prepared::PreparedWorld,
    retained_protocol::{
        EntityKind, Operation, Viewport, WorldDefinition, WorldLayerKind, WorldOrigin, validate_id,
    },
    retained_world::{ProjectedCell, World},
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
    /// Non-covering shadow tiles, painted through the same tile path.
    Shadows,
}

/// One world layer, in paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapWorldLayer<'a> {
    pub id: &'a str,
    /// The draw band: higher layers paint over lower ones.
    pub layer: i32,
    pub kind: MapLayerKind,
}

impl<'a> MapWorldLayer<'a> {
    pub(crate) fn get_from_definition(
        layer: &'a crate::retained_prepared::PreparedWorldLayer,
    ) -> Self {
        Self {
            id: &layer.id,
            layer: layer.layer,
            kind: match layer.kind {
                WorldLayerKind::Gameplay => MapLayerKind::Gameplay,
                WorldLayerKind::Decoration => MapLayerKind::Decoration,
                WorldLayerKind::Tiles => MapLayerKind::Tiles,
                WorldLayerKind::Shadows => MapLayerKind::Shadows,
            },
        }
    }
}

/// A map world prepared for painting.
#[derive(Clone)]
pub struct MapWorld {
    id: String,
    prepared: Arc<PreparedWorld>,
}

impl MapWorld {
    pub(crate) fn get_from_prepared(id: String, prepared: Arc<PreparedWorld>) -> Self {
        Self { id, prepared }
    }

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
        self.prepared
            .layers
            .iter()
            .map(MapWorldLayer::get_from_definition)
    }

    /// Whether the graphical field hides the glyph its owner layer shows at
    /// this cell, because a usable tile covers it.
    pub fn has_covering_tile(&self, column: u32, row: u32, owner_layer_id: &str) -> bool {
        self.prepared.has_covering_tile(column, row, owner_layer_id)
    }

    /// Gameplay glyphs left visible by the accepted composition, including object coverage.
    pub fn get_shown_glyph_cells(&self) -> Vec<(u32, u32)> {
        let mut cells = Vec::new();
        let (columns, rows) = self.get_size();
        for row in 0..rows {
            let Some(source) = self.prepared.source.get_row(row) else {
                continue;
            };
            for column in 0..columns {
                let cell = &source.cells[column as usize];
                if !cell.glyph.trim().is_empty()
                    && self.prepared.layers.iter().any(|layer| {
                        layer.id == cell.owner_layer_id && layer.kind == WorldLayerKind::Gameplay
                    })
                    && !self.has_covering_tile(column, row, &cell.owner_layer_id)
                {
                    cells.push((column, row));
                }
            }
        }
        cells
    }

    /// Paints one tiles layer's visible tiles through `camera`, filling its
    /// parent. Nothing for an unknown or glyph layer.
    pub fn paint_tiles(
        &self,
        layer_id: &str,
        camera: MapCamera,
        samples: &MapTileSamples,
    ) -> Option<impl IntoElement + use<>> {
        if !self
            .prepared
            .layers
            .iter()
            .any(|layer| layer.id == layer_id && layer.kind == WorldLayerKind::Tiles)
        {
            return None;
        }
        self.paint_layer(layer_id, camera, samples)
    }

    /// Paints a tiles or shadows layer through the runtime tile painter.
    /// Call in get_layers() order; glyph layers remain owned by the host.
    pub fn paint_layer(
        &self,
        layer_id: &str,
        camera: MapCamera,
        samples: &MapTileSamples,
    ) -> Option<impl IntoElement + use<>> {
        let index = self
            .prepared
            .layers
            .iter()
            .position(|layer| layer.id == layer_id && layer.kind.has_tile_cells())?;
        Some(crate::retained_paint::tile_element(
            self.prepared.clone(),
            index,
            ViewportTransform::fit(camera.width, camera.height, camera.width, camera.height),
            camera.get_viewport(self.get_cell_size()),
            samples.0.clone(),
        ))
    }

    /// Paints any world layer through the same painter as the game. Call in
    /// get_layers() order. `field_font` is the font size fitted to one unscaled
    /// world cell; font family is inherited from the host element.
    pub fn paint_world_layer(
        &self,
        layer_id: &str,
        camera: MapCamera,
        field_font: f32,
        samples: &MapTileSamples,
    ) -> Option<gpui::AnyElement> {
        self.paint_world_layer_excluding_cells(
            layer_id,
            camera,
            field_font,
            samples,
            &HashSet::new(),
        )
    }

    /// Paints a world layer while leaving authored preview cells to the host.
    /// Exclusions are (column, row) in world coordinates and apply only to
    /// composed glyph/background cells, never tiles, shadows or tile covering.
    /// The prepared world remains unchanged.
    pub fn paint_world_layer_excluding_cells(
        &self,
        layer_id: &str,
        camera: MapCamera,
        field_font: f32,
        samples: &MapTileSamples,
        excluded_cells: &HashSet<(u32, u32)>,
    ) -> Option<gpui::AnyElement> {
        let index = self
            .prepared
            .layers
            .iter()
            .position(|layer| layer.id == layer_id)?;
        let view = camera.get_viewport(self.get_cell_size());
        let projected = self.get_projected_cells(index, &view, excluded_cells);
        Some(crate::retained_paint::world_layer_element(
            &self.prepared,
            index,
            &view,
            &projected,
            ViewportTransform::fit(camera.width, camera.height, camera.width, camera.height),
            field_font,
            &samples.0,
        ))
    }

    fn get_projected_cells(
        &self,
        layer_index: usize,
        view: &Viewport,
        excluded_cells: &HashSet<(u32, u32)>,
    ) -> Arc<Vec<ProjectedCell>> {
        if self.prepared.layers[layer_index].kind.has_tile_cells() {
            return Arc::new(Vec::new());
        }
        crate::retained_paint::project_world_excluding_cells(&self.prepared, view, excluded_cells)
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
    pub(crate) fn get_viewport(&self, (cell_width, cell_height): (f32, f32)) -> Viewport {
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
    use crate::{
        protocol::Grid,
        retained_paint::{cell_bounds, project_world, should_paint_world_cell},
        retained_prepared::PreparedScene,
        retained_state::RetainedSession,
    };
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

    #[test]
    fn engine_composed_world_shares_runtime_and_editor_glyphs_projection_and_tile_pixels() {
        // Generated by tests/fixtures/compose-engine-world.php through
        // PresentationWorld::getFromLayers and MapGraphics::getShownGlyphCells.
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/engine-world.json")).unwrap();
        let directory = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(32, 32, image::Rgba([10, 20, 30, 255]))
            .save(directory.path().join("floor.png"))
            .unwrap();
        let assets = MapAssets::new(directory.path()).unwrap();
        let operations = fixture["operations"].as_array().unwrap().clone();
        let editor = MapWorld::from_operations(operations.clone(), &assets).unwrap();
        let grid = Grid {
            columns: 20,
            rows: 10,
            cell_width: 10,
            cell_height: 20,
        };
        let mut session = RetainedSession::default();
        let accepted = session
            .apply(
                serde_json::from_value(json!({
                    "frame":1,"baseGeneration":0,"generation":1,"reset":true,"present":true,
                    "operations":operations,
                }))
                .unwrap(),
                grid,
            )
            .unwrap();
        let production = PreparedScene::prepare(accepted.scene, 1, grid, &assets.0, None).unwrap();
        let runtime = production.get_world("map").unwrap();
        assert_eq!(
            fixture["shownGlyphs"],
            json!([{"x":1,"y":0,"glyph":".","layer":"terrain"}])
        );
        for (left, top, scale) in [(0.0, 0.0, 1.0), (11.0, 3.0, 0.5), (-27.0, -5.0, 1.5)] {
            let camera = MapCamera {
                left,
                top,
                scale,
                width: 240.0,
                height: 96.0,
                tile_frame: 0,
            };
            let view = camera.get_viewport(editor.get_cell_size());
            let transform =
                ViewportTransform::fit(camera.width, camera.height, camera.width, camera.height);
            let paint_cells = |world: &PreparedWorld| {
                let mut painted = Vec::new();
                for layer in world
                    .layers
                    .iter()
                    .filter(|layer| !layer.kind.has_tile_cells())
                {
                    for &cell in project_world(world, &view).iter() {
                        let source = &world.source.get_row(cell.world_row).unwrap().cells
                            [cell.world_column as usize];
                        if should_paint_world_cell(world, &layer.id, cell, source) {
                            painted.push((
                                layer.id.clone(),
                                cell.world_column,
                                cell.world_row,
                                source.glyph.clone(),
                                cell_bounds(
                                    cell,
                                    world.source.cell_size(),
                                    (0.0, 0.0, 48.0),
                                    transform,
                                    &view,
                                ),
                            ));
                        }
                    }
                }
                painted
            };
            let shown = paint_cells(runtime);
            assert_eq!(shown, paint_cells(&editor.prepared));
            assert_eq!(shown.len(), 1);
            assert_eq!(
                (&shown[0].0, shown[0].1, &shown[0].3),
                (&"map:terrain".to_string(), 1, &".".to_string())
            );
            assert_eq!(
                runtime.get_tile_image(0, 0).unwrap().as_bytes(0),
                editor.prepared.get_tile_image(0, 0).unwrap().as_bytes(0)
            );
            for layer in editor.get_layers() {
                assert!(
                    editor
                        .paint_world_layer(layer.id, camera, 20.0, &MapTileSamples::default())
                        .is_some()
                );
            }
            assert!(
                editor
                    .paint_world_layer("unknown", camera, 20.0, &MapTileSamples::default())
                    .is_none()
            );
        }
    }

    fn covered(world: &MapWorld) -> Vec<(u32, u32)> {
        (0..2)
            .flat_map(|row| (0..3).map(move |column| (column, row)))
            .filter(|&(column, row)| world.has_covering_tile(column, row, "map:terrain"))
            .collect()
    }

    #[test]
    fn canonical_shadow_tiles_share_projection_without_covering_glyphs() {
        let (_directory, assets) = assets();
        let mut put = world_put(true);
        put["value"]["layers"].as_array_mut().unwrap().push(json!({
            "id":"tiles:floor:shadows","layer":-100,"kind":"shadows"
        }));
        put["value"]["tileset"]["tiles"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "width":1,"left":1,"top":-1,"frames":[[
                    {"fill":[0,0,0,64],"width":1,"height":2,"left":0,"top":0}
                ]]
            }));
        let shadow = json!({"op":"worldTiles","id":"map","layerId":"tiles:floor:shadows",
            "rows":[{"row":0,"cells":[{"column":1,"tile":1}]}]});
        let world =
            MapWorld::from_operations(vec![put, rows(), tiles(0, &[0]), shadow], &assets).unwrap();
        assert_eq!(covered(&world), [(0, 0)]);
        assert!(world.prepared.get_tile_image(1, 0).is_some());
        assert_eq!(
            world.get_layers().map(|layer| layer.id).collect::<Vec<_>>(),
            [
                "tiles:floor",
                "tiles:floor:shadows",
                "map:terrain",
                "map:detail"
            ]
        );
        let view = camera(-12.0, 0.0).get_viewport(world.get_cell_size());
        let mut projected = Vec::new();
        world
            .prepared
            .source
            .project_visible_tiles("tiles:floor:shadows", &view, |cell, tile| {
                projected.push((cell.world_column, cell.screen_column, tile))
            });
        assert_eq!(projected, [(1, 0, 1)]);
        assert_eq!(
            world.get_layers().nth(1).unwrap().kind,
            MapLayerKind::Shadows
        );
        let samples = MapTileSamples::default();
        assert!(
            world
                .paint_layer("tiles:floor:shadows", camera(0.0, 0.0), &samples)
                .is_some()
        );
        assert!(
            world
                .paint_layer("tiles:floor", camera(0.0, 0.0), &samples)
                .is_some()
        );
        assert!(
            world
                .paint_layer("map:terrain", camera(0.0, 0.0), &samples)
                .is_none()
        );
        assert!(
            world
                .paint_layer("unknown", camera(0.0, 0.0), &samples)
                .is_none()
        );
        assert!(
            world
                .paint_tiles("tiles:floor:shadows", camera(0.0, 0.0), &samples)
                .is_none()
        );
        let cleared = world
            .update(
                vec![json!({"op":"worldTiles","id":"map",
                    "layerId":"tiles:floor:shadows","rows":[{"row":0,"cells":[]}]
                })],
                &assets,
            )
            .unwrap();
        assert_eq!(
            cleared
                .prepared
                .source
                .get_tile("tiles:floor:shadows", 1, 0),
            None
        );
        assert_eq!(covered(&cleared), [(0, 0)]);
        assert!(Arc::ptr_eq(
            world.prepared.tileset.as_ref().unwrap(),
            cleared.prepared.tileset.as_ref().unwrap()
        ));
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

    fn get_painted_cells(
        world: &MapWorld,
        layer_id: &str,
        camera: MapCamera,
        excluded: &HashSet<(u32, u32)>,
    ) -> Vec<ProjectedCell> {
        let index = world
            .prepared
            .layers
            .iter()
            .position(|layer| layer.id == layer_id)
            .unwrap();
        let view = camera.get_viewport(world.get_cell_size());
        world
            .get_projected_cells(index, &view, excluded)
            .iter()
            .copied()
            .filter(|cell| {
                let source = &world.prepared.source.get_row(cell.world_row).unwrap().cells
                    [cell.world_column as usize];
                should_paint_world_cell(&world.prepared, layer_id, *cell, source)
            })
            .collect()
    }

    #[test]
    fn authored_preview_excludes_foreground_and_blank_colored_background_without_mutating_source() {
        let (_directory, assets) = assets();
        let mut colored = rows();
        colored["rows"][0]["cells"][0] = json!({
            "glyph":"#","foreground":{"kind":"rgb","r":120,"g":80,"b":40},"ownerLayerId":"map:terrain"
        });
        colored["rows"][0]["cells"][1] = json!({
            "glyph":" ","background":{"kind":"rgb","r":10,"g":20,"b":30},"ownerLayerId":"map:terrain"
        });
        let world = MapWorld::from_operations(vec![world_put(false), colored], &assets).unwrap();
        let camera = camera(0.0, 0.0);
        let empty = HashSet::new();
        let original = get_painted_cells(&world, "map:terrain", camera, &empty);
        assert_eq!(original.len(), 6);
        let excluded = HashSet::from([(0, 0), (1, 0)]);
        let masked = get_painted_cells(&world, "map:terrain", camera, &excluded);
        assert_eq!(masked.len(), 4);
        assert!(
            masked
                .iter()
                .all(|cell| !excluded.contains(&(cell.world_column, cell.world_row)))
        );
        assert_eq!(
            get_painted_cells(&world, "map:terrain", camera, &empty),
            original
        );
        let row = world.prepared.source.get_row(0).unwrap();
        assert_eq!(row.cells[0].glyph, "#");
        assert_eq!(row.cells[0].foreground.as_ref().unwrap().rgb(), 0x785028);
        assert_eq!(row.cells[1].glyph, " ");
        assert_eq!(row.cells[1].background.as_ref().unwrap().rgb(), 0x0a141e);
        let samples = MapTileSamples::default();
        assert!(
            world
                .paint_world_layer_excluding_cells("map:terrain", camera, 20.0, &samples, &excluded)
                .is_some()
        );
        assert!(
            world
                .paint_world_layer_excluding_cells("map:detail", camera, 20.0, &samples, &excluded)
                .is_some()
        );
    }

    #[test]
    fn preview_exclusion_uses_world_coordinates_after_pan_and_ignores_offscreen_cells() {
        let (_directory, assets) = assets();
        let world = MapWorld::from_operations(vec![world_put(false), rows()], &assets).unwrap();
        let camera = MapCamera {
            left: -12.0,
            top: -24.0,
            scale: 0.5,
            width: 12.0,
            height: 24.0,
            tile_frame: 0,
        };
        let original = get_painted_cells(&world, "map:terrain", camera, &HashSet::new());
        assert!(original.contains(&ProjectedCell {
            world_column: 1,
            world_row: 1,
            screen_column: 0,
            screen_row: 0
        }));
        assert_eq!(
            get_painted_cells(
                &world,
                "map:terrain",
                camera,
                &HashSet::from([(0, 0), (999, 999)])
            ),
            original
        );
        assert_eq!(
            get_painted_cells(&world, "map:terrain", camera, &HashSet::from([(1, 1)])),
            original
                .iter()
                .copied()
                .filter(|cell| (cell.world_column, cell.world_row) != (1, 1))
                .collect::<Vec<_>>()
        );
        let panned = MapCamera {
            left: 9.0,
            top: 3.0,
            width: 80.0,
            height: 80.0,
            ..camera
        };
        let unmasked = get_painted_cells(&world, "map:terrain", panned, &HashSet::new());
        let masked = get_painted_cells(&world, "map:terrain", panned, &HashSet::from([(1, 1)]));
        assert_eq!(
            masked,
            unmasked
                .into_iter()
                .filter(|cell| (cell.world_column, cell.world_row) != (1, 1))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cell_exclusion_keeps_tiles_shadows_overhang_and_covering_unchanged() {
        let (_directory, assets) = assets();
        let mut put = world_put(true);
        put["value"]["layers"].as_array_mut().unwrap().extend([
            json!({"id":"tiles:floor:shadows","layer":-100,"kind":"shadows"}),
            json!({"id":"tiles:floor:above","layer":900,"kind":"tiles"}),
        ]);
        put["value"]["tileset"]["tiles"].as_array_mut().unwrap().extend([
            json!({"width":1,"left":1,"top":-1,"frames":[[{"fill":[0,0,0,64],"width":1,"height":2,"left":0,"top":0}]]}),
            json!({"width":2,"left":-1,"top":-1,"frames":[[{"fill":[30,60,90,255],"width":2,"height":2,"left":0,"top":0}]]}),
        ]);
        let world = MapWorld::from_operations(vec![
            put, rows(), tiles(0, &[0]),
            json!({"op":"worldTiles","id":"map","layerId":"tiles:floor:shadows","rows":[{"row":0,"cells":[{"column":1,"tile":1}]}]}),
            json!({"op":"worldTiles","id":"map","layerId":"tiles:floor:above","rows":[{"row":1,"cells":[{"column":2,"tile":2}]}]}),
        ], &assets).unwrap();
        let prepared = world.prepared.clone();
        let images = (0..3)
            .map(|tile| world.prepared.get_tile_image(tile, 0).unwrap().clone())
            .collect::<Vec<_>>();
        let camera = camera(-9.0, 0.0);
        let view = camera.get_viewport(world.get_cell_size());
        let get_tiles = || {
            let mut cells = Vec::new();
            for layer in world
                .get_layers()
                .filter(|layer| matches!(layer.kind, MapLayerKind::Tiles | MapLayerKind::Shadows))
            {
                world
                    .prepared
                    .source
                    .project_visible_tiles(layer.id, &view, |cell, tile| {
                        cells.push((layer.id.to_string(), cell, tile));
                    });
            }
            cells
        };
        let original = get_tiles();
        assert!(
            original
                .iter()
                .any(|(id, _, tile)| id == "tiles:floor:above" && *tile == 2)
        );
        assert!(
            original
                .iter()
                .any(|(id, _, tile)| id == "tiles:floor:shadows" && *tile == 1)
        );
        let covering = covered(&world);
        let excluded = (0..2)
            .flat_map(|row| (0..3).map(move |column| (column, row)))
            .collect::<HashSet<_>>();
        let samples = MapTileSamples::default();
        for (index, layer) in world
            .prepared
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| layer.kind.has_tile_cells())
        {
            assert_eq!(
                world.get_projected_cells(index, &view, &excluded),
                world.get_projected_cells(index, &view, &HashSet::new())
            );
            assert!(
                world
                    .paint_world_layer_excluding_cells(&layer.id, camera, 20.0, &samples, &excluded)
                    .is_some()
            );
        }
        assert_eq!(get_tiles(), original);
        assert_eq!(covered(&world), covering);
        assert!(Arc::ptr_eq(&world.prepared, &prepared));
        for (tile, image) in images.iter().enumerate() {
            assert!(Arc::ptr_eq(
                world.prepared.get_tile_image(tile as u32, 0).unwrap(),
                image
            ));
        }
        assert!(get_painted_cells(&world, "map:terrain", camera, &excluded).is_empty());
    }

    #[test]
    fn empty_preview_exclusion_matches_original_projection_and_public_layer_support() {
        let (_directory, assets) = assets();
        let world =
            MapWorld::from_operations(vec![world_put(true), rows(), tiles(0, &[0])], &assets)
                .unwrap();
        let samples = MapTileSamples::default();
        for camera in [camera(0.0, 0.0), camera(11.0, 3.0), camera(-27.0, -5.0)] {
            let view = camera.get_viewport(world.get_cell_size());
            let empty = HashSet::new();
            for (index, layer) in world.prepared.layers.iter().enumerate() {
                if !layer.kind.has_tile_cells() {
                    assert_eq!(
                        world.get_projected_cells(index, &view, &empty),
                        project_world(&world.prepared, &view)
                    );
                }
                let mut original = world
                    .paint_world_layer(&layer.id, camera, 20.0, &samples)
                    .unwrap();
                let mut explicit = world
                    .paint_world_layer_excluding_cells(&layer.id, camera, 20.0, &samples, &empty)
                    .unwrap();
                assert_eq!(
                    original.downcast_mut::<gpui::Div>().is_some(),
                    explicit.downcast_mut::<gpui::Div>().is_some()
                );
            }
            assert!(
                world
                    .paint_world_layer("unknown", camera, 20.0, &samples)
                    .is_none()
            );
            assert!(
                world
                    .paint_world_layer_excluding_cells("unknown", camera, 20.0, &samples, &empty)
                    .is_none()
            );
        }
    }
}
