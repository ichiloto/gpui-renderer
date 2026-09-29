//! The v2 session update envelope. A frame is a bounded transaction against
//! retained presentation state, not a replacement of every visible element.
use crate::color::ColorSpec;
use crate::protocol::{Grid, ViewportPoint, ViewportRect};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_WORLD_LAYERS: usize = 64;
pub const MAX_WORLD_CELLS: usize = 1_048_576;
/// Most tile cells across all tiles layers of one world.
pub const MAX_WORLD_TILE_CELLS: usize = 1_048_576;
/// Tileset bounds. The renderer knows tiles only as frames of pieces copied
/// from sheets; every layout and composition rule stays with the producer.
pub const MAX_TILE_SIZE: u32 = 256;
pub const MAX_TILESET_SHEETS: usize = 16;
pub const MAX_TILESET_TILES: usize = 8192;
pub const MAX_TILE_FRAMES: usize = 4;
pub const MAX_TILE_PIECES: usize = 8;
/// Source-state charge for one tile piece.
pub const TILE_PIECE_BYTES: usize = 32;
/// Largest field cell side, in logical pixels. The field unit is its own
/// pitch, independent of the session text grid that UI text uses.
pub const MAX_WORLD_CELL_SIZE: u32 = 256;
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
    /// One world cell's width in logical pixels, before viewport scale. A
    /// world cell is one terminal cell, so it is usually taller than wide.
    pub cell_width: u32,
    /// One world cell's height in logical pixels, before viewport scale.
    pub cell_height: u32,
    pub layers: Vec<WorldLayer>,
    /// Graphics for `tiles` layers. Absent for a glyph-only world.
    #[serde(default)]
    pub tileset: Option<Tileset>,
}

/// A catalog of tiles, each composed from pieces of sheet images. A tile is
/// `tileSize` source pixels tall and drawn one world cell tall; its own width
/// and left offset (in source pixels, from its cell's left edge) let it be a
/// slice narrower than a cell or overhang the cells beside it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tileset {
    /// Side of one composed tile in source pixels.
    pub tile_size: u32,
    /// Asset-root-relative sheet images that pieces index.
    pub sheets: Vec<PathBuf>,
    pub tiles: Vec<TileDefinition>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileDefinition {
    /// Width in source pixels, 1 to `tileSize`; `tileSize` when absent.
    #[serde(default)]
    pub width: Option<u32>,
    /// Source pixels from the cell's left edge to the tile's, -`tileSize` to
    /// `tileSize`.
    #[serde(default)]
    pub left: i32,
    /// Source pixels from the cell's top edge to the tile's, -`tileSize` to
    /// `tileSize`.
    #[serde(default)]
    pub top: i32,
    /// Animation frames; each is its pieces in paint order.
    pub frames: Vec<Vec<TilePiece>>,
}

impl TileDefinition {
    pub fn get_width(&self, tile_size: u32) -> u32 {
        self.width.unwrap_or(tile_size)
    }
}

/// A source rectangle of one sheet copied unscaled to (left, top) of the tile.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TilePiece {
    pub sheet: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub top: u32,
}

impl Tileset {
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=MAX_TILE_SIZE).contains(&self.tile_size) || !self.tile_size.is_multiple_of(2) {
            return Err("tileset tileSize must be an even 2..256 pixels".into());
        }
        if self.sheets.is_empty() || self.sheets.len() > MAX_TILESET_SHEETS {
            return Err("tileset requires 1..16 sheets".into());
        }
        for sheet in &self.sheets {
            if sheet.as_os_str().is_empty()
                || sheet.is_absolute()
                || sheet.as_os_str().len() > crate::protocol::MAX_TILE_ASSET_BYTES
            {
                return Err("tileset sheets must be nonempty relative asset paths".into());
            }
        }
        if self.tiles.is_empty() || self.tiles.len() > MAX_TILESET_TILES {
            return Err("tileset requires 1..8192 tiles".into());
        }
        let size = u64::from(self.tile_size);
        for tile in &self.tiles {
            let width = tile.get_width(self.tile_size);
            if width == 0
                || width > self.tile_size
                || tile.left.unsigned_abs() > self.tile_size
                || tile.top.unsigned_abs() > self.tile_size
            {
                return Err(
                    "tileset tile width must be 1..tileSize and left and top within tileSize"
                        .into(),
                );
            }
            if tile.frames.is_empty() || tile.frames.len() > MAX_TILE_FRAMES {
                return Err("tileset tiles require 1..4 frames".into());
            }
            for frame in &tile.frames {
                if frame.is_empty() || frame.len() > MAX_TILE_PIECES {
                    return Err("tileset tile frames require 1..8 pieces".into());
                }
                for piece in frame {
                    if piece.sheet as usize >= self.sheets.len()
                        || piece.width == 0
                        || piece.height == 0
                        || u64::from(piece.left) + u64::from(piece.width) > u64::from(width)
                        || u64::from(piece.top) + u64::from(piece.height) > size
                    {
                        return Err(
                            "tileset piece must name a sheet and fit inside its tile".into()
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Source-state estimate: a fixed charge per piece plus the sheet paths.
    pub fn estimated_bytes(&self) -> usize {
        let pieces: usize = self
            .tiles
            .iter()
            .flat_map(|tile| &tile.frames)
            .map(Vec::len)
            .sum();
        pieces * TILE_PIECE_BYTES
            + self
                .sheets
                .iter()
                .map(|sheet| sheet.as_os_str().len())
                .sum::<usize>()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorldLayerKind {
    Gameplay,
    Decoration,
    /// Graphics from the world's tileset; owns no glyphs.
    Tiles,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldLayer {
    pub id: String,
    pub layer: i32,
    pub kind: WorldLayerKind,
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
        for size in [self.cell_width, self.cell_height] {
            if size == 0 || size > MAX_WORLD_CELL_SIZE {
                return Err("world cellWidth and cellHeight must be 1..256 logical pixels".into());
            }
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
            if layer.kind == WorldLayerKind::Tiles && self.tileset.is_none() {
                return Err("a tiles world layer requires the world's tileset".into());
            }
        }
        if let Some(tileset) = &self.tileset {
            tileset.validate()?;
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
    pub owner_layer_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldTileRow {
    pub row: u32,
    pub cells: Vec<WorldTileCell>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorldTileCell {
    pub column: u32,
    /// Index into the world tileset's tiles.
    pub tile: u32,
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
    /// Animation counter; a tile with N frames shows frame tileFrame % N.
    #[serde(default)]
    pub tile_frame: u32,
    /// `field_motion`: the sprite the camera follows while it slides.
    #[serde(default)]
    pub follow: Option<ViewportFollow>,
}

/// The field sprite the camera follows. When the world origin changes in
/// the frame that sprite slides a step, the camera slides with it on the
/// same clock; the named text layers are drawn relative to that sprite.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewportFollow {
    pub sprite_id: String,
    #[serde(default)]
    pub text_layer_ids: Vec<String>,
}

/// Longest step a sprite may slide over, in seconds.
pub const MAX_SPRITE_MOTION_SECONDS: f64 = 60.0;

/// `field_motion`: how a retained field sprite reached its current cell, one
/// step presented as a slide from the cell it last stood on.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpriteMotion {
    /// Seconds the slide takes.
    pub duration: f64,
}

impl SpriteMotion {
    pub fn validate(self) -> Result<Self, String> {
        if !self.duration.is_finite()
            || self.duration <= 0.0
            || self.duration > MAX_SPRITE_MOTION_SECONDS
        {
            return Err("sprite motion duration must be more than 0 and at most 60 seconds".into());
        }
        Ok(self)
    }
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
        if let Some(follow) = &self.follow
            && (self.world_id.is_none()
                || !self.sprite_ids.contains(&follow.sprite_id)
                || follow
                    .text_layer_ids
                    .iter()
                    .any(|id| !self.text_layer_ids.contains(id)))
        {
            return Err(
                "viewport follow requires a world and names only its own sprites and text layers"
                    .into(),
            );
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
