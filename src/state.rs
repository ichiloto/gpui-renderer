use crate::assets::AssetRoot;
use crate::protocol::{Frame, Grid, Hello, Sprite};
use gpui::RenderImage;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct PreparedSprite {
    pub sprite: Sprite,
    pub image: Arc<RenderImage>,
}

#[derive(Debug)]
pub struct PreparedFrame {
    pub number: u64,
    pub text: Vec<String>,
    pub sprites: Vec<PreparedSprite>,
}

impl PreparedFrame {
    pub fn prepare(frame: Frame, grid: Grid, assets: &AssetRoot) -> Result<Self, String> {
        frame.validate(grid)?;
        let mut images = HashMap::new();
        let mut sprites = Vec::with_capacity(frame.sprites.len());
        let mut decoded_bytes = 0;
        for sprite in frame.sprites {
            let image = if let Some(image) = images.get(&sprite.asset) {
                Arc::clone(image)
            } else {
                let image = assets.load(&sprite.asset)?;
                decoded_bytes += image.as_bytes(0).map_or(0, |bytes| bytes.len());
                if decoded_bytes > 64 * 1024 * 1024 {
                    return Err("frame exceeds 64 MiB decoded image limit".into());
                }
                images.insert(sprite.asset.clone(), image.clone());
                image
            };
            sprites.push(PreparedSprite { sprite, image });
        }
        // Stable sort: equal layers keep their order in the protocol array.
        sprites.sort_by_key(|item| item.sprite.layer);
        Ok(Self {
            number: frame.frame,
            text: frame.text,
            sprites,
        })
    }
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
}
