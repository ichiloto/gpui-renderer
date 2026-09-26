//! The v2 session update envelope. A frame is a bounded transaction against
//! retained presentation state, not a replacement of every visible element.
use crate::color::ColorSpec;
use crate::protocol::{Grid, ViewportPoint, ViewportRect};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_WORLD_LAYERS: usize = 64;
pub const MAX_WORLD_CELLS: usize = 1_048_576;
pub const MAX_WORLD_TILE_CELLS: usize = 1_048_576;
pub const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_STAGING_AND_VISIBLE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameUpdate {
    pub frame: u64,
    pub base_generation: u64,
    pub generation: u64,
    #[serde(default)]
    pub reset: bool,
    #[serde(default = "present_by_default")]
    pub present: bool,
    #[serde(default)]
    pub operations: Vec<Operation>,
    #[serde(default, deserialize_with = "viewport_change")]
    pub viewport: ViewportChange,
}

fn present_by_default() -> bool {
    true
}

impl FrameUpdate {
    pub fn validate_envelope(&self) -> Result<(), String> {
        if self.generation == 0 || (!self.reset && self.generation <= self.base_generation) {
            return Err(
                "frame generation must increase beyond baseGeneration unless resetting".into(),
            );
        }
        if self.operations.len() > 4096 {
            return Err("frame exceeds 4096 retained operations".into());
        }
        if !self.present && !matches!(self.viewport, ViewportChange::Keep) {
            return Err("staged frame cannot change the visible viewport".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub enum ViewportChange {
    #[default]
    Keep,
    Clear,
    Set(Viewport),
}

fn viewport_change<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ViewportChange, D::Error> {
    Option::<Viewport>::deserialize(d)
        .map(|value| value.map_or(ViewportChange::Clear, ViewportChange::Set))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Put {
        kind: EntityKind,
        id: String,
        value: Value,
    },
    Remove {
        kind: EntityKind,
        id: String,
    },
    WorldRows {
        id: String,
        rows: Vec<WorldRow>,
    },
    WorldTiles {
        id: String,
        layer_id: String,
        rows: Vec<WorldTileRow>,
    },
    TextRows {
        id: String,
        rows: Vec<TextRow>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    World,
    Text,
    Sprite,
    Canvas,
    CanvasImage,
    CanvasIndicator,
    CanvasText,
    CanvasComposite,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldDefinition {
    pub columns: u32,
    pub rows: u32,
    pub layers: Vec<WorldLayer>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorldLayerKind {
    Gameplay,
    Decoration,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldLayer {
    pub id: String,
    pub layer: i32,
    pub kind: WorldLayerKind,
    #[serde(default)]
    pub asset: Option<PathBuf>,
    #[serde(default)]
    pub sources: Vec<crate::protocol::SourceRect>,
}

impl WorldDefinition {
    pub fn validate(&self) -> Result<(), String> {
        if self.columns == 0
            || self.rows == 0
            || self.columns > 16384
            || self.rows > 16384
            || u64::from(self.columns) * u64::from(self.rows) > MAX_WORLD_CELLS as u64
        {
            return Err("world dimensions exceed 16384 axes or 1048576 cells".into());
        }
        if self.layers.is_empty() || self.layers.len() > MAX_WORLD_LAYERS {
            return Err("world requires 1..64 layers".into());
        }
        let mut ids = std::collections::HashSet::new();
        for layer in &self.layers {
            validate_id(&layer.id)?;
            if !ids.insert(layer.id.as_str()) {
                return Err("world layer ids must be unique".into());
            }
            if layer.sources.len() > crate::protocol::MAX_BATCH_SOURCES {
                return Err("world layer exceeds 256 tile sources".into());
            }
            match (&layer.asset, layer.sources.is_empty()) {
                (None, true) => {}
                (Some(asset), false)
                    if !asset.as_os_str().is_empty()
                        && !asset.is_absolute()
                        && asset.as_os_str().len() <= crate::protocol::MAX_TILE_ASSET_BYTES => {}
                _ => return Err("world tile atlas and source catalog must appear together".into()),
            }
            for source in &layer.sources {
                source.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldRow {
    pub row: u32,
    pub cells: Vec<WorldCell>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldCell {
    pub glyph: String,
    pub foreground: Option<ColorSpec>,
    pub background: Option<ColorSpec>,
    pub display_width: u8,
    pub owner_layer_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldTileRow {
    pub row: u32,
    pub cells: Vec<WorldTileCell>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldTileCell {
    pub column: u32,
    pub source: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRow {
    pub row: u32,
    pub runs: Vec<crate::protocol::TextRun>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenText {
    pub id: String,
    pub layer: i32,
    pub order: u32,
    #[serde(default)]
    pub runs: Vec<crate::protocol::TextRun>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasRoot {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub background: Option<ColorSpec>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorldOrigin {
    pub column: i32,
    pub row: i32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Viewport {
    pub scale: f32,
    pub origin: ViewportPoint,
    #[serde(default)]
    pub world_origin: WorldOrigin,
    pub clip_rect: ViewportRect,
    #[serde(default)]
    pub world_id: Option<String>,
    #[serde(default)]
    pub text_layer_ids: Vec<String>,
    #[serde(default)]
    pub sprite_ids: Vec<String>,
}

impl Viewport {
    pub fn validate(&self, grid: Grid) -> Result<(), String> {
        let width = (grid.columns * grid.cell_width) as f32;
        let height = (grid.rows * grid.cell_height) as f32;
        let clip = self.clip_rect;
        if !self.scale.is_finite()
            || self.scale <= 0.0
            || self.scale > crate::protocol::MAX_RENDER_SCALE
            || ![
                self.origin.x,
                self.origin.y,
                clip.x,
                clip.y,
                clip.width,
                clip.height,
            ]
            .into_iter()
            .all(f32::is_finite)
            || self.origin.x.abs() > width * crate::protocol::MAX_RENDER_SCALE
            || self.origin.y.abs() > height * crate::protocol::MAX_RENDER_SCALE
            || self.world_origin.column.unsigned_abs() > 16384
            || self.world_origin.row.unsigned_abs() > 16384
            || clip.x < 0.0
            || clip.y < 0.0
            || clip.width <= 0.0
            || clip.height <= 0.0
            || clip.x + clip.width > width
            || clip.y + clip.height > height
        {
            return Err("retained viewport must be finite and clipped inside the session".into());
        }
        if let Some(id) = &self.world_id {
            validate_id(id)?;
        } else if self.world_origin != WorldOrigin::default() {
            return Err("a nonzero worldOrigin requires worldId".into());
        }
        for ids in [&self.text_layer_ids, &self.sprite_ids] {
            let mut seen = std::collections::HashSet::new();
            for id in ids {
                validate_id(id)?;
                if !seen.insert(id) {
                    return Err("viewport member ids must be unique".into());
                }
            }
        }
        Ok(())
    }
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err("retained ids must be nonempty, control-free and at most 256 bytes".into());
    }
    Ok(())
}
