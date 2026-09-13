use crate::assets::AssetRoot;
use crate::protocol::{Frame, FrameV2, Grid, Hello, Sprite, TextLayer, TileBatch};
use gpui::RenderImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub struct PreparedSprite {
    pub sprite: Sprite,
    pub image: Arc<RenderImage>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PaintItem {
    LegacyText,
    Text(usize),
    Sprite(usize),
    Tiles(usize),
}

#[derive(Debug)]
pub struct PreparedFrame {
    pub observation: Option<crate::diagnostics::FrameTrace>,
    pub number: u64,
    pub text: Vec<String>,
    pub sprites: Vec<PreparedSprite>,
    pub text_layers: Vec<TextLayer>,
    pub plan: Vec<PaintItem>,
    pub cached_images: Vec<Arc<RenderImage>>,
    pub tile_batches: Vec<Arc<PreparedTileBatch>>,
    pub resources: FrameResources,
}

#[derive(Debug)]
pub struct PreparedTileBatch {
    pub batch: TileBatch,
    pub regions: Vec<Arc<RenderImage>>,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct FrameResources {
    pub tile_batches: usize,
    pub tile_cells: usize,
    pub decoded_images: usize,
    pub decoded_bytes: usize,
    pub prepared_regions: usize,
    pub region_bytes: usize,
    pub png_decodes: u64,
    pub region_builds: u64,
}

/// Shared source-image accounting across the independent actor and terrain paths.
struct FrameImages<'a> {
    assets: &'a AssetRoot,
    sources: HashMap<PathBuf, Arc<RenderImage>>,
    regions: HashMap<gpui::ImageId, Arc<RenderImage>>,
    resources: FrameResources,
    before: (u64, u64),
}
impl<'a> FrameImages<'a> {
    fn new(assets: &'a AssetRoot) -> Self {
        Self {
            assets,
            sources: HashMap::new(),
            regions: HashMap::new(),
            resources: FrameResources::default(),
            before: assets.preparation_totals(),
        }
    }
    fn load(&mut self, path: &Path) -> Result<Arc<RenderImage>, String> {
        let canonical = self.assets.resolve(path)?;
        if let Some(image) = self.sources.get(&canonical) {
            return Ok(image.clone());
        }
        if self.sources.len() == 1024 {
            return Err("frame exceeds 1024 unique decoded images".into());
        }
        let image = self.assets.load(path)?;
        self.resources.decoded_bytes += image.as_bytes(0).unwrap().len();
        if self.resources.decoded_bytes > crate::assets::MAX_DECODED_BYTES {
            return Err("frame exceeds 64 MiB decoded image limit".into());
        }
        self.sources.insert(canonical, image.clone());
        Ok(image)
    }
    fn region(
        &mut self,
        atlas: &Arc<RenderImage>,
        rect: crate::protocol::SourceRect,
    ) -> Result<Arc<RenderImage>, String> {
        let image = self.assets.prepare_region(atlas, rect)?;
        if !self.regions.contains_key(&image.id) {
            self.resources.region_bytes += image.as_bytes(0).unwrap().len();
            if self.resources.region_bytes > crate::tile_regions::MAX_REGION_BYTES {
                return Err("frame exceeds 64 MiB prepared-region limit".into());
            }
            self.regions.insert(image.id, image.clone());
        }
        Ok(image)
    }
    fn finish(mut self) -> (Vec<Arc<RenderImage>>, FrameResources) {
        let after = self.assets.preparation_totals();
        self.resources.png_decodes = after.0 - self.before.0;
        self.resources.region_builds = after.1 - self.before.1;
        self.resources.decoded_images = self.sources.len();
        self.resources.prepared_regions = self.regions.len();
        // Include live images evicted during preparation, not just the final cache
        // generation. The UI must track and eventually retire those GPU entries too.
        let mut images = HashMap::new();
        for image in self
            .sources
            .into_values()
            .chain(self.regions.into_values())
            .chain(self.assets.cached_images())
            .chain(self.assets.cached_regions())
        {
            images.insert(image.id, image);
        }
        (images.into_values().collect(), self.resources)
    }
}

impl PreparedFrame {
    pub fn prepare(frame: Frame, grid: Grid, assets: &AssetRoot) -> Result<Self, String> {
        frame.validate(grid)?;
        let mut images = FrameImages::new(assets);
        let mut sprites = prepare_sprites(frame.sprites, &mut images)?;
        sprites.sort_by_key(|item| item.sprite.layer);
        let plan = std::iter::once(PaintItem::LegacyText)
            .chain((0..sprites.len()).map(PaintItem::Sprite))
            .collect();
        let (cached_images, resources) = images.finish();
        Ok(Self {
            observation: None,
            number: frame.frame,
            text: frame.text,
            text_layers: vec![],
            sprites,
            tile_batches: vec![],
            plan,
            cached_images,
            resources,
        })
    }

    pub fn prepare_v2(frame: FrameV2, grid: Grid, assets: &AssetRoot) -> Result<Self, String> {
        frame.validate(grid)?;
        let mut images = FrameImages::new(assets);
        let sprites = prepare_sprites(frame.sprites, &mut images)?;
        let mut tile_batches = Vec::new();
        for batch in frame.tile_batches.unwrap_or_default() {
            let atlas = images.load(&batch.asset)?;
            let size = atlas.size(0);
            // Validate every catalog source before deriving any region, even unused ones.
            for rect in &batch.sources {
                rect.validate_image(size.width.0 as u32, size.height.0 as u32)?;
            }
            let regions = batch
                .sources
                .iter()
                .map(|rect| images.region(&atlas, *rect))
                .collect::<Result<Vec<_>, _>>()?;
            images.resources.tile_cells += batch.cells.len();
            tile_batches.push(Arc::new(PreparedTileBatch { batch, regions }));
        }
        images.resources.tile_batches = tile_batches.len();
        // Stable sort retains incoming order and the existing text-before-sprite tie.
        let mut plan: Vec<_> = (0..tile_batches.len())
            .map(PaintItem::Tiles)
            .chain((0..frame.text_layers.len()).map(PaintItem::Text))
            .chain((0..sprites.len()).map(PaintItem::Sprite))
            .collect();
        plan.sort_by_key(|item| match *item {
            PaintItem::Tiles(i) => tile_batches[i].batch.layer,
            PaintItem::Text(i) => frame.text_layers[i].layer,
            PaintItem::Sprite(i) => sprites[i].sprite.layer,
            PaintItem::LegacyText => unreachable!(),
        });
        let (cached_images, resources) = images.finish();
        Ok(Self {
            observation: None,
            number: frame.frame,
            text: vec![],
            text_layers: frame.text_layers,
            sprites,
            tile_batches,
            plan,
            cached_images,
            resources,
        })
    }
}

fn prepare_sprites(
    source: Vec<Sprite>,
    images: &mut FrameImages<'_>,
) -> Result<Vec<PreparedSprite>, String> {
    source
        .into_iter()
        .map(|sprite| {
            let image = images.load(&sprite.asset)?;
            if let Some(rect) = sprite.source_rect {
                let size = image.size(0);
                rect.validate_image(size.width.0 as u32, size.height.0 as u32)?;
            }
            Ok(PreparedSprite { sprite, image })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub struct TextCell {
    pub row: u32,
    pub column: u32,
    pub glyph: Option<char>,
    pub foreground: u32,
    pub background: u32,
}

/// Includes explicit spaces; absent cells yield nothing. Overlapping runs retain order.
pub fn painted_cells(layer: &TextLayer) -> impl Iterator<Item = TextCell> + '_ {
    layer.runs.iter().flat_map(|run| {
        run.text
            .chars()
            .enumerate()
            .map(move |(offset, glyph)| TextCell {
                row: run.row,
                column: run.column + offset as u32,
                glyph: (glyph != ' ').then_some(glyph),
                foreground: run
                    .foreground
                    .as_ref()
                    .map_or(crate::color::DEFAULT_FOREGROUND, |c| c.rgb()),
                background: run
                    .background
                    .as_ref()
                    .map_or(crate::color::DEFAULT_BACKGROUND, |c| c.rgb()),
            })
    })
}

pub struct RendererState {
    pub hello: Hello,
    pub frame: Option<PreparedFrame>,
}

impl RendererState {
    pub fn replace(&mut self, frame: PreparedFrame) {
        self.frame = Some(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Anchor;

    fn tiles_fixture() -> FrameV2 {
        let crate::protocol::Message::FrameV2(frame) = crate::protocol::parse(include_bytes!(
            "../fixtures/tile-batches/valid-full-viewport.json"
        ))
        .unwrap()
        .message
        else {
            panic!()
        };
        frame
    }

    #[test]
    fn full_terrain_viewport_shares_atlas_with_all_1024_actors_and_reuses_regions() {
        let (mut state, assets) = setup();
        state.hello.grid = Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        };
        let mut source = tiles_fixture();
        let actor = source.sprites[0].clone();
        source.sprites = (0..1024)
            .map(|i| Sprite {
                id: format!("actor-{i}"),
                ..actor.clone()
            })
            .collect();
        let first = PreparedFrame::prepare_v2(source.clone(), state.hello.grid, &assets).unwrap();
        assert_eq!(first.resources.tile_cells, 4860);
        assert_eq!(first.resources.tile_batches, 1);
        assert_eq!(first.resources.decoded_images, 1);
        assert_eq!(first.resources.png_decodes, 1);
        assert_eq!(first.resources.prepared_regions, 2);
        assert_eq!(first.resources.region_builds, 2);
        assert_eq!(first.resources.decoded_bytes, 32 * 48 * 4);
        assert_eq!(first.resources.region_bytes, 2 * (16 + 4) * (24 + 4) * 4);
        assert_eq!(
            first
                .plan
                .iter()
                .filter(|p| matches!(p, PaintItem::Tiles(_)))
                .count(),
            1
        );
        let region = first.tile_batches[0].regions[0].clone();
        state.replace(first);
        source.tile_batches.as_mut().unwrap()[0].cells[0].source = 1;
        source.sprites[0].source_rect.as_mut().unwrap().x = 16;
        let changed = PreparedFrame::prepare_v2(source, state.hello.grid, &assets).unwrap();
        assert_eq!(
            (
                changed.resources.png_decodes,
                changed.resources.region_builds
            ),
            (0, 0)
        );
        assert!(Arc::ptr_eq(&region, &changed.tile_batches[0].regions[0]));
        assert!(Arc::ptr_eq(
            &changed.sprites[0].image,
            &state.frame.as_ref().unwrap().sprites[0].image
        ));
        assert_eq!(changed.tile_batches[0].batch.cells[0].source, 1);
    }

    #[test]
    fn terrain_text_sprite_ties_are_stable_and_scene_snapshots_clear_terrain() {
        let (mut state, assets) = setup();
        state.hello.grid = Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        };
        let mut source = tiles_fixture();
        source.tile_batches.as_mut().unwrap()[0].layer = 0;
        let mut second = source.tile_batches.as_ref().unwrap()[0].clone();
        second.id = "second".into();
        source.tile_batches.as_mut().unwrap().push(second);
        source.sprites[0].layer = 0;
        let full = PreparedFrame::prepare_v2(source.clone(), state.hello.grid, &assets).unwrap();
        assert_eq!(
            full.plan,
            [
                PaintItem::Tiles(0),
                PaintItem::Tiles(1),
                PaintItem::Text(0),
                PaintItem::Sprite(0),
                PaintItem::Text(1)
            ]
        );
        let owned = full.tile_batches[0].clone();
        state.replace(full);
        for tiles in [None, Some(vec![])] {
            let mut menu = source.clone();
            menu.tile_batches = tiles;
            menu.sprites.clear();
            state.replace(PreparedFrame::prepare_v2(menu, state.hello.grid, &assets).unwrap());
            assert!(state.frame.as_ref().unwrap().tile_batches.is_empty());
            assert!(
                !state
                    .frame
                    .as_ref()
                    .unwrap()
                    .plan
                    .iter()
                    .any(|p| matches!(p, PaintItem::Tiles(_)))
            );
        }
        // Earlier queued/displayed snapshots retain their immutable storage safely.
        assert_eq!(owned.batch.cells.len(), 4860);
        assert!(!owned.regions[0].as_bytes(0).unwrap().is_empty());
        let restored = PreparedFrame::prepare_v2(source, state.hello.grid, &assets).unwrap();
        assert!(Arc::ptr_eq(
            &owned.regions[0],
            &restored.tile_batches[0].regions[0]
        ));
    }

    #[test]
    fn tile_changes_participate_in_complete_snapshot_equality() {
        let original = tiles_fixture();
        for change in 0..6 {
            let mut candidate = original.clone();
            let batch = &mut candidate.tile_batches.as_mut().unwrap()[0];
            match change {
                0 => batch.id = "new".into(),
                1 => batch.asset = "other.png".into(),
                2 => batch.layer -= 1,
                3 => batch.sources[0].x += 1,
                4 => batch.cells[0].source = 1,
                _ => batch.cells.swap(0, 1),
            }
            assert_ne!(original, candidate);
        }
    }

    #[test]
    fn shared_image_count_budget_includes_terrain_atlases_after_actors() {
        let dir = tempfile::tempdir().unwrap();
        let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        for i in 0..1025 {
            pixel.save(dir.path().join(format!("{i}.png"))).unwrap();
        }
        let assets = AssetRoot::new(dir.path()).unwrap();
        let mut source = tiles_fixture();
        source.sprites = (0..1024)
            .map(|i| Sprite {
                asset: format!("{i}.png").into(),
                ..sprite(&format!("actor-{i}"), 100)
            })
            .collect();
        let batch = &mut source.tile_batches.as_mut().unwrap()[0];
        batch.sources = vec![crate::protocol::SourceRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }];
        for cell in &mut batch.cells {
            cell.source = 0;
        }
        batch.asset = "./0.png".into();
        let grid = Grid {
            columns: 135,
            rows: 36,
            cell_width: 10,
            cell_height: 20,
        };
        let accepted = PreparedFrame::prepare_v2(source.clone(), grid, &assets).unwrap();
        assert_eq!(accepted.resources.decoded_images, 1024);
        source.tile_batches.as_mut().unwrap()[0].asset = "1024.png".into();
        assert!(
            PreparedFrame::prepare_v2(source, grid, &assets)
                .unwrap_err()
                .contains("1024 unique")
        );
        assert_eq!(accepted.sprites.len(), 1024);
    }

    #[test]
    fn decoded_byte_budget_is_shared_between_actor_images_and_tile_atlases() {
        let dir = tempfile::tempdir().unwrap();
        let large = image::RgbaImage::from_pixel(4096, 2048, image::Rgba([10, 20, 30, 255]));
        large.save(dir.path().join("large.png")).unwrap();
        std::fs::copy(dir.path().join("large.png"), dir.path().join("second.png")).unwrap();
        drop(large);
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]))
            .save(dir.path().join("pixel.png"))
            .unwrap();
        let assets = AssetRoot::new(dir.path()).unwrap();
        let mut images = FrameImages::new(&assets);
        images.load(Path::new("large.png")).unwrap();
        images.load(Path::new("second.png")).unwrap();
        assert_eq!(
            images.resources.decoded_bytes,
            crate::assets::MAX_DECODED_BYTES
        );
        // Both paths use this same frame accounting; aliases do not spend twice.
        images.load(Path::new("./large.png")).unwrap();
        assert!(
            images
                .load(Path::new("pixel.png"))
                .unwrap_err()
                .contains("64 MiB decoded")
        );
        assert_eq!(images.sources.len(), 2);
    }
    fn setup() -> (RendererState, AssetRoot) {
        let asset_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let assets = AssetRoot::new(&asset_root).unwrap();
        (
            RendererState {
                hello: Hello {
                    title: "Home".into(),
                    required_capabilities: vec![],
                    asset_root,
                    grid: Grid {
                        columns: 80,
                        rows: 24,
                        cell_width: 16,
                        cell_height: 24,
                    },
                },
                frame: None,
            },
            assets,
        )
    }
    fn sprite(id: &str, layer: i32) -> Sprite {
        Sprite {
            id: id.into(),
            asset: "test-sprite.png".into(),
            x: 8,
            y: 4,
            width: 32,
            height: 48,
            anchor: Anchor::BottomCenter,
            layer,
            source_rect: None,
        }
    }
    #[test]
    fn distinct_crops_share_full_sheet_across_frames_and_keep_layering() {
        use crate::protocol::SourceRect;
        let (mut state, _) = setup();
        let dir = tempfile::tempdir().unwrap();
        image::RgbaImage::from_fn(4, 2, |x, y| {
            image::Rgba(if x < 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, if y == 0 { 128 } else { 0 }]
            })
        })
        .save(dir.path().join("sheet.png"))
        .unwrap();
        let assets = AssetRoot::new(dir.path()).unwrap();
        let mut left = sprite("left", 100);
        left.asset = "sheet.png".into();
        left.source_rect = Some(SourceRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        let mut right = left.clone();
        right.id = "right".into();
        right.source_rect.as_mut().unwrap().x = 2;
        let mut frame = v2();
        frame.sprites = vec![left.clone(), right.clone()];
        let first = PreparedFrame::prepare_v2(frame.clone(), state.hello.grid, &assets).unwrap();
        assert_eq!(
            first.plan,
            [
                PaintItem::Text(0),
                PaintItem::Sprite(0),
                PaintItem::Sprite(1),
                PaintItem::Text(1)
            ]
        );
        assert!(Arc::ptr_eq(
            &first.sprites[0].image,
            &first.sprites[1].image
        ));
        let sheet = first.sprites[0].image.clone();
        assert_eq!(sheet.as_bytes(0).unwrap().len(), 4 * 2 * 4);
        assert_eq!(&sheet.as_bytes(0).unwrap()[0..4], &[0, 0, 255, 255]);
        assert_eq!(&sheet.as_bytes(0).unwrap()[8..12], &[255, 0, 0, 128]);
        state.replace(first);
        frame.sprites[0].source_rect = right.source_rect;
        // Same frame number and only the crop changes; a complete replacement still applies.
        state.replace(PreparedFrame::prepare_v2(frame, state.hello.grid, &assets).unwrap());
        let current = state.frame.as_ref().unwrap();
        assert_eq!(current.sprites[0].sprite.source_rect, right.source_rect);
        assert!(Arc::ptr_eq(&sheet, &current.sprites[0].image));
        assert_eq!(current.cached_images.len(), 1);
        let full = PreparedFrame::prepare(
            Frame {
                frame: 2,
                text: vec![],
                sprites: vec![Sprite {
                    source_rect: None,
                    ..left
                }],
            },
            state.hello.grid,
            &assets,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&sheet, &full.sprites[0].image));
        assert!(full.sprites[0].sprite.source_rect.is_none());
        assert_eq!(full.plan, [PaintItem::LegacyText, PaintItem::Sprite(0)]);
    }

    #[test]
    fn crop_bounds_use_the_decoded_image_and_rejection_preserves_snapshot() {
        use crate::protocol::SourceRect;
        let (mut state, assets) = setup();
        let mut frame = v2();
        frame.sprites[0].source_rect = Some(SourceRect {
            x: 16,
            y: 24,
            width: 16,
            height: 24,
        });
        state.replace(PreparedFrame::prepare_v2(frame.clone(), state.hello.grid, &assets).unwrap());
        let image = state.frame.as_ref().unwrap().sprites[0].image.clone();
        for bad in [
            SourceRect {
                x: 17,
                y: 24,
                width: 16,
                height: 24,
            },
            SourceRect {
                x: 16,
                y: 25,
                width: 16,
                height: 24,
            },
            SourceRect {
                x: 32,
                y: 0,
                width: 1,
                height: 1,
            },
            SourceRect {
                x: 0,
                y: 48,
                width: 1,
                height: 1,
            },
            SourceRect {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
        ] {
            frame.sprites[0].source_rect = Some(bad);
            frame.frame = 2;
            assert!(PreparedFrame::prepare_v2(frame.clone(), state.hello.grid, &assets).is_err());
            assert!(
                PreparedFrame::prepare(
                    Frame {
                        frame: 2,
                        text: vec![],
                        sprites: frame.sprites.clone()
                    },
                    state.hello.grid,
                    &assets
                )
                .is_err()
            );
            assert_eq!(state.frame.as_ref().unwrap().number, 1);
            assert!(Arc::ptr_eq(
                &image,
                &state.frame.as_ref().unwrap().sprites[0].image
            ));
        }
    }
    #[test]
    fn full_frames_clear_previous_text_and_sprites() {
        let (mut state, assets) = setup();
        state.replace(
            PreparedFrame::prepare(
                Frame {
                    frame: 1,
                    text: vec!["Home".into()],
                    sprites: vec![sprite("test", 100)],
                },
                state.hello.grid,
                &assets,
            )
            .unwrap(),
        );
        state.replace(
            PreparedFrame::prepare(
                Frame {
                    frame: 2,
                    text: vec![],
                    sprites: vec![],
                },
                state.hello.grid,
                &assets,
            )
            .unwrap(),
        );
        let current = state.frame.unwrap();
        assert_eq!(current.number, 2);
        assert!(current.sprites.is_empty());
        assert!(current.text.is_empty());
    }
    #[test]
    fn rejected_frame_preserves_last_snapshot() {
        let (mut state, assets) = setup();
        state.replace(
            PreparedFrame::prepare(
                Frame {
                    frame: 7,
                    text: vec!["Home".into()],
                    sprites: vec![sprite("test", 100)],
                },
                state.hello.grid,
                &assets,
            )
            .unwrap(),
        );
        let mut bad = sprite("bad", 0);
        bad.asset = "missing.png".into();
        let result = PreparedFrame::prepare(
            Frame {
                frame: 8,
                text: vec!["changed".into()],
                sprites: vec![sprite("valid", 0), bad],
            },
            state.hello.grid,
            &assets,
        );
        assert!(result.is_err());
        assert_eq!(state.frame.as_ref().unwrap().number, 7);
        assert_eq!(state.frame.as_ref().unwrap().text, ["Home"]);
    }
    #[test]
    fn layers_are_ascending_and_ties_stable() {
        let (state, assets) = setup();
        let frame = PreparedFrame::prepare(
            Frame {
                frame: 1,
                text: vec![],
                sprites: vec![
                    sprite("front", 100),
                    sprite("first", -2),
                    sprite("second", -2),
                ],
            },
            state.hello.grid,
            &assets,
        )
        .unwrap();
        assert_eq!(
            frame
                .sprites
                .iter()
                .map(|s| s.sprite.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "front"]
        );
    }
    #[test]
    fn short_rows_are_allowed_but_overflow_and_controls_are_rejected() {
        let (state, assets) = setup();
        for text in [vec!["ok".into()], vec![], vec!["".into()]] {
            assert!(
                PreparedFrame::prepare(
                    Frame {
                        frame: 1,
                        text,
                        sprites: vec![]
                    },
                    state.hello.grid,
                    &assets
                )
                .is_ok()
            );
        }
        for text in [
            vec!["x".repeat(81)],
            vec!["x".into(); 25],
            vec!["\u{1b}[31m".into()],
        ] {
            assert!(
                PreparedFrame::prepare(
                    Frame {
                        frame: 1,
                        text,
                        sprites: vec![]
                    },
                    state.hello.grid,
                    &assets
                )
                .is_err()
            );
        }
    }
    fn layer(id: &str, z: i32) -> TextLayer {
        TextLayer {
            id: id.into(),
            layer: z,
            runs: vec![crate::protocol::TextRun {
                row: 4,
                column: 2,
                text: "A B".into(),
                foreground: None,
                background: None,
            }],
        }
    }
    fn v2() -> FrameV2 {
        FrameV2 {
            tile_batches: None,
            frame: 1,
            text_layers: vec![layer("world", 0), layer("ui", 1000)],
            sprites: vec![sprite("player", 100)],
        }
    }
    #[test]
    fn text_cells_have_exact_positions_and_opaque_spaces() {
        let text = layer("sparse", 0);
        let cells: Vec<_> = painted_cells(&text).collect();
        assert_eq!(
            cells
                .iter()
                .map(|c| (c.row, c.column, c.glyph))
                .collect::<Vec<_>>(),
            [(4, 2, Some('A')), (4, 3, None), (4, 4, Some('B'))]
        );
        assert!(
            cells
                .iter()
                .all(|c| c.background == crate::color::DEFAULT_BACKGROUND
                    && c.foreground == crate::color::DEFAULT_FOREGROUND)
        );
        let mut text = text;
        text.runs.push(crate::protocol::TextRun {
            row: 4,
            column: 3,
            text: " ".into(),
            foreground: None,
            background: Some(crate::color::ColorSpec::Ansi256 { index: 196 }),
        });
        let last = painted_cells(&text).last().unwrap();
        assert_eq!(
            (last.row, last.column, last.glyph, last.background),
            (4, 3, None, 0xff0000)
        );
    }
    #[test]
    fn themed_ansi_backgrounds_and_literal_colours_are_preserved_even_for_spaces() {
        use crate::color::{ANSI16, ColorSpec};
        let text = TextLayer {
            id: "selection".into(),
            layer: 1000,
            runs: vec![
                crate::protocol::TextRun {
                    row: 0,
                    column: 0,
                    text: "A ".into(),
                    foreground: Some(ColorSpec::Ansi16 { index: 0 }),
                    background: Some(ColorSpec::Ansi16 { index: 4 }),
                },
                crate::protocol::TextRun {
                    row: 0,
                    column: 2,
                    text: "B".into(),
                    foreground: Some(ColorSpec::Rgb { r: 0, g: 0, b: 128 }),
                    background: Some(ColorSpec::Rgb { r: 1, g: 2, b: 3 }),
                },
                crate::protocol::TextRun {
                    row: 0,
                    column: 3,
                    text: " ".into(),
                    foreground: None,
                    background: Some(ColorSpec::Ansi256 { index: 21 }),
                },
            ],
        };
        let cells: Vec<_> = painted_cells(&text).collect();
        assert_eq!((cells[0].foreground, cells[0].background), (0, ANSI16[4]));
        assert_eq!((cells[1].glyph, cells[1].background), (None, ANSI16[4]));
        assert_eq!(
            (cells[2].foreground, cells[2].background),
            (0x000080, 0x010203)
        );
        assert_eq!((cells[3].glyph, cells[3].background), (None, 0x0000ff));
    }
    #[test]
    fn unified_plan_orders_world_sprite_ui_and_all_ties_stably() {
        let (state, assets) = setup();
        let frame = PreparedFrame::prepare_v2(v2(), state.hello.grid, &assets).unwrap();
        assert_eq!(
            frame.plan,
            [PaintItem::Text(0), PaintItem::Sprite(0), PaintItem::Text(1)]
        );
        let frame = FrameV2 {
            tile_batches: None,
            frame: 1,
            text_layers: vec![
                layer("text-first", -10),
                layer("text-second", -10),
                layer("below", i32::MIN),
            ],
            sprites: vec![
                sprite("first", -10),
                sprite("second", -10),
                sprite("highest", i32::MAX),
            ],
        };
        let frame = PreparedFrame::prepare_v2(frame, state.hello.grid, &assets).unwrap();
        assert_eq!(
            frame.plan,
            [
                PaintItem::Text(2),
                PaintItem::Text(0),
                PaintItem::Text(1),
                PaintItem::Sprite(0),
                PaintItem::Sprite(1),
                PaintItem::Sprite(2)
            ]
        );
    }
    #[test]
    fn v1_negative_sprite_layer_still_paints_after_text() {
        let (state, assets) = setup();
        let frame = PreparedFrame::prepare(
            Frame {
                frame: 1,
                text: vec!["x".into()],
                sprites: vec![sprite("negative", i32::MIN)],
            },
            state.hello.grid,
            &assets,
        )
        .unwrap();
        assert_eq!(frame.plan, [PaintItem::LegacyText, PaintItem::Sprite(0)]);
    }
    #[test]
    fn same_number_style_only_replacement_preserves_all_presentation_intent() {
        let (mut state, assets) = setup();
        let frame = v2();
        state.replace(PreparedFrame::prepare_v2(frame.clone(), state.hello.grid, &assets).unwrap());
        let mut changed = frame.clone();
        changed.text_layers[0].runs[0].foreground =
            Some(crate::color::ColorSpec::Ansi16 { index: 9 });
        changed.text_layers[0].runs[0].background = Some(crate::color::ColorSpec::Rgb {
            r: 10,
            g: 20,
            b: 30,
        });
        assert_ne!(frame, changed);
        state.replace(
            PreparedFrame::prepare_v2(changed.clone(), state.hello.grid, &assets).unwrap(),
        );
        assert_eq!(
            state.frame.as_ref().unwrap().text_layers,
            changed.text_layers
        );
        assert_eq!(state.frame.as_ref().unwrap().number, 1);
        changed.text_layers[0].id = "renamed".into();
        changed.text_layers[0].layer = 2000;
        changed.sprites[0].x = 9;
        state.replace(
            PreparedFrame::prepare_v2(changed.clone(), state.hello.grid, &assets).unwrap(),
        );
        assert_eq!(
            state.frame.as_ref().unwrap().text_layers,
            changed.text_layers
        );
        assert_eq!(state.frame.as_ref().unwrap().sprites[0].sprite.x, 9);
        assert_eq!(
            state.frame.as_ref().unwrap().plan,
            [PaintItem::Sprite(0), PaintItem::Text(1), PaintItem::Text(0)]
        );
    }
    #[test]
    fn invalid_v2_style_geometry_or_asset_preserves_last_accepted_frame() {
        let (mut state, assets) = setup();
        let source = v2();
        state
            .replace(PreparedFrame::prepare_v2(source.clone(), state.hello.grid, &assets).unwrap());
        for failure in 0..3 {
            let mut bad = source.clone();
            bad.frame = 2;
            match failure {
                0 => bad.text_layers[0].runs[0].column = 999,
                1 => bad.text_layers[0].runs[0].text = "\n".into(),
                _ => bad.sprites[0].asset = "missing.png".into(),
            }
            assert!(PreparedFrame::prepare_v2(bad, state.hello.grid, &assets).is_err());
            assert_eq!(state.frame.as_ref().unwrap().number, 1);
            assert_eq!(
                state.frame.as_ref().unwrap().text_layers,
                source.text_layers
            );
        }
    }
    #[test]
    fn empty_v2_replacement_clears_every_layer_and_sprite() {
        let (mut state, assets) = setup();
        state.replace(PreparedFrame::prepare_v2(v2(), state.hello.grid, &assets).unwrap());
        state.replace(
            PreparedFrame::prepare_v2(
                FrameV2 {
                    tile_batches: None,
                    frame: 2,
                    text_layers: vec![],
                    sprites: vec![],
                },
                state.hello.grid,
                &assets,
            )
            .unwrap(),
        );
        let frame = state.frame.unwrap();
        assert!(frame.plan.is_empty());
        assert!(frame.text_layers.is_empty());
        assert!(frame.sprites.is_empty());
    }
}
