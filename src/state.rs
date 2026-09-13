use crate::assets::AssetRoot;
use crate::protocol::{Frame, FrameV2, Grid, Hello, Sprite, TextLayer};
use gpui::RenderImage;
use std::collections::HashMap;
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
}

impl PreparedFrame {
    pub fn prepare(frame: Frame, grid: Grid, assets: &AssetRoot) -> Result<Self, String> {
        frame.validate(grid)?;
        let mut sprites = prepare_sprites(frame.sprites, assets)?;
        sprites.sort_by_key(|item| item.sprite.layer);
        let plan = std::iter::once(PaintItem::LegacyText)
            .chain((0..sprites.len()).map(PaintItem::Sprite))
            .collect();
        Ok(Self {
            observation: None,
            number: frame.frame,
            text: frame.text,
            text_layers: vec![],
            sprites,
            plan,
            cached_images: assets.cached_images(),
        })
    }

    pub fn prepare_v2(frame: FrameV2, grid: Grid, assets: &AssetRoot) -> Result<Self, String> {
        frame.validate(grid)?;
        let sprites = prepare_sprites(frame.sprites, assets)?;
        // Stable sort over text followed by sprites gives the documented tie order.
        let mut plan: Vec<_> = (0..frame.text_layers.len())
            .map(PaintItem::Text)
            .chain((0..sprites.len()).map(PaintItem::Sprite))
            .collect();
        plan.sort_by_key(|item| match *item {
            PaintItem::Text(i) => frame.text_layers[i].layer,
            PaintItem::Sprite(i) => sprites[i].sprite.layer,
            PaintItem::LegacyText => unreachable!(),
        });
        Ok(Self {
            observation: None,
            number: frame.frame,
            text: vec![],
            text_layers: frame.text_layers,
            sprites,
            plan,
            cached_images: assets.cached_images(),
        })
    }
}

fn prepare_sprites(source: Vec<Sprite>, assets: &AssetRoot) -> Result<Vec<PreparedSprite>, String> {
    let mut images = HashMap::new();
    let mut sprites = Vec::with_capacity(source.len());
    let mut decoded_bytes = 0;
    for sprite in source {
        let image = if let Some(image) = images.get(&sprite.asset) {
            Arc::clone(image)
        } else {
            let image = assets.load(&sprite.asset)?;
            decoded_bytes += image.as_bytes(0).map_or(0, |bytes| bytes.len());
            if decoded_bytes > crate::assets::MAX_DECODED_BYTES {
                return Err("frame exceeds 64 MiB decoded image limit".into());
            }
            images.insert(sprite.asset.clone(), image.clone());
            image
        };
        if let Some(rect) = sprite.source_rect {
            let size = image.size(0);
            // Decoder limits guarantee positive dimensions at most 4096.
            rect.validate_image(size.width.0 as u32, size.height.0 as u32)?;
        }
        sprites.push(PreparedSprite { sprite, image });
    }
    Ok(sprites)
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
